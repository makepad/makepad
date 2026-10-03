//! std::io: Read/Write/BufRead/Seek, the in-memory and buffered adapters, stdio.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

mod buffered;
mod cursor;
mod error;
mod stdio;

pub use self::buffered::{BufReader, BufWriter, IntoInnerError, LineWriter};
pub use self::cursor::Cursor;
pub use self::error::{decode_error_kind, Error, ErrorKind, RawOsError, Result};
pub use self::stdio::{_eprint, _print, cleanup, stderr, stdin, stdout, IsTerminal, Stderr, StderrLock, Stdin, StdinLock, Stdout, StdoutLock};

pub mod prelude {
    pub use super::{BufRead, Read, Seek, Write};
}

const DEFAULT_BUF_SIZE: usize = 8 * 1024;

pub trait Read {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        default_read_to_end(self, buf)
    }

    fn read_to_string(&mut self, buf: &mut String) -> Result<usize> {
        let mut bytes = Vec::new();
        let n = self.read_to_end(&mut bytes)?;
        match core::str::from_utf8(&bytes) {
            Ok(s) => {
                buf.push_str(s);
                Ok(n)
            }
            Err(_) => Err(Error::INVALID_UTF8),
        }
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        let mut off = 0;
        while off < buf.len() {
            match self.read(&mut buf[off..]) {
                Ok(0) => return Err(Error::READ_EXACT_EOF),
                Ok(n) => off += n,
                Err(e) => {
                    if !e.is_interrupted() {
                        return Err(e);
                    }
                }
            }
        }
        Ok(())
    }

    fn by_ref(&mut self) -> &mut Self
    where
        Self: Sized,
    {
        self
    }

    fn bytes(self) -> Bytes<Self>
    where
        Self: Sized,
    {
        Bytes { inner: self }
    }

    fn take(self, limit: u64) -> Take<Self>
    where
        Self: Sized,
    {
        Take { inner: self, limit }
    }

    fn chain<R: Read>(self, next: R) -> Chain<Self, R>
    where
        Self: Sized,
    {
        Chain { first: self, second: next, done_first: false }
    }
}

fn default_read_to_end<R: Read + ?Sized>(r: &mut R, buf: &mut Vec<u8>) -> Result<usize> {
    let start = buf.len();
    let mut chunk = [0u8; DEFAULT_BUF_SIZE];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => return Ok(buf.len() - start),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => {
                if !e.is_interrupted() {
                    return Err(e);
                }
            }
        }
    }
}

/// Adapter so `write!` on an io::Write keeps the first io error.
struct FmtAdapter<'a, W: Write + ?Sized> {
    inner: &'a mut W,
    error: Result<()>,
}

impl<'a, W: Write + ?Sized> fmt::Write for FmtAdapter<'a, W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        match self.inner.write_all(s.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.error = Err(e);
                Err(fmt::Error)
            }
        }
    }
}

pub trait Write {
    fn write(&mut self, buf: &[u8]) -> Result<usize>;
    fn flush(&mut self) -> Result<()>;

    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        let mut rest = buf;
        while !rest.is_empty() {
            match self.write(rest) {
                Ok(0) => return Err(Error::WRITE_ALL_EOF),
                Ok(n) => rest = &rest[n..],
                Err(e) => {
                    if !e.is_interrupted() {
                        return Err(e);
                    }
                }
            }
        }
        Ok(())
    }

    fn write_fmt(&mut self, args: fmt::Arguments) -> Result<()> {
        let mut out = FmtAdapter { inner: self, error: Ok(()) };
        match fmt::write(&mut out, args) {
            Ok(()) => Ok(()),
            Err(_) => {
                if out.error.is_err() {
                    out.error
                } else {
                    panic!("a formatting trait implementation returned an error when the underlying stream did not");
                }
            }
        }
    }

    fn by_ref(&mut self) -> &mut Self
    where
        Self: Sized,
    {
        self
    }
}

#[derive(Copy, PartialEq, Eq, Clone, Debug)]
pub enum SeekFrom {
    Start(u64),
    End(i64),
    Current(i64),
}

pub trait Seek {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64>;

    fn rewind(&mut self) -> Result<()> {
        self.seek(SeekFrom::Start(0))?;
        Ok(())
    }

    fn stream_position(&mut self) -> Result<u64> {
        self.seek(SeekFrom::Current(0))
    }

    fn seek_relative(&mut self, offset: i64) -> Result<()> {
        self.seek(SeekFrom::Current(offset))?;
        Ok(())
    }
}

pub trait BufRead: Read {
    fn fill_buf(&mut self) -> Result<&[u8]>;
    fn consume(&mut self, amt: usize);

    fn has_data_left(&mut self) -> Result<bool> {
        let b = self.fill_buf()?;
        Ok(!b.is_empty())
    }

    fn read_until(&mut self, byte: u8, buf: &mut Vec<u8>) -> Result<usize> {
        read_until(self, byte, buf)
    }

    fn skip_until(&mut self, byte: u8) -> Result<usize> {
        let mut sink = Vec::new();
        read_until(self, byte, &mut sink)
    }

    fn read_line(&mut self, buf: &mut String) -> Result<usize> {
        let mut bytes = Vec::new();
        let n = read_until(self, b'\n', &mut bytes)?;
        match core::str::from_utf8(&bytes) {
            Ok(s) => {
                buf.push_str(s);
                Ok(n)
            }
            Err(_) => Err(Error::INVALID_UTF8),
        }
    }

    fn split(self, byte: u8) -> Split<Self>
    where
        Self: Sized,
    {
        Split { buf: self, delim: byte }
    }

    fn lines(self) -> Lines<Self>
    where
        Self: Sized,
    {
        Lines { buf: self }
    }
}

fn read_until<R: BufRead + ?Sized>(r: &mut R, delim: u8, buf: &mut Vec<u8>) -> Result<usize> {
    let mut read = 0;
    loop {
        let (done, used) = {
            let available = match r.fill_buf() {
                Ok(n) => n,
                Err(e) => {
                    if e.is_interrupted() {
                        continue;
                    }
                    return Err(e);
                }
            };
            let mut i = 0;
            while i < available.len() && available[i] != delim {
                i += 1;
            }
            if i < available.len() {
                buf.extend_from_slice(&available[..=i]);
                (true, i + 1)
            } else {
                buf.extend_from_slice(available);
                (false, available.len())
            }
        };
        r.consume(used);
        read += used;
        if done || used == 0 {
            return Ok(read);
        }
    }
}

pub struct Bytes<R> {
    inner: R,
}

impl<R: Read> Iterator for Bytes<R> {
    type Item = Result<u8>;
    fn next(&mut self) -> Option<Result<u8>> {
        let mut byte = [0u8; 1];
        loop {
            return match self.inner.read(&mut byte) {
                Ok(0) => None,
                Ok(_) => Some(Ok(byte[0])),
                Err(e) => {
                    if e.is_interrupted() {
                        continue;
                    }
                    Some(Err(e))
                }
            };
        }
    }
}

pub struct Take<T> {
    inner: T,
    limit: u64,
}

impl<T> Take<T> {
    pub fn limit(&self) -> u64 {
        self.limit
    }
    pub fn set_limit(&mut self, limit: u64) {
        self.limit = limit;
    }
    pub fn into_inner(self) -> T {
        self.inner
    }
    pub fn get_ref(&self) -> &T {
        &self.inner
    }
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T: Read> Read for Take<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.limit == 0 {
            return Ok(0);
        }
        let max = if (buf.len() as u64) < self.limit { buf.len() } else { self.limit as usize };
        let n = self.inner.read(&mut buf[..max])?;
        if n as u64 > self.limit {
            panic!("number of read bytes exceeds limit");
        }
        self.limit -= n as u64;
        Ok(n)
    }
}

impl<T: BufRead> BufRead for Take<T> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        if self.limit == 0 {
            return Ok(&[]);
        }
        let limit = self.limit;
        let buf = self.inner.fill_buf()?;
        let cap = if (buf.len() as u64) < limit { buf.len() } else { limit as usize };
        Ok(&buf[..cap])
    }
    fn consume(&mut self, amt: usize) {
        let amt = if (amt as u64) < self.limit { amt } else { self.limit as usize };
        self.limit -= amt as u64;
        self.inner.consume(amt);
    }
}

pub struct Chain<T, U> {
    first: T,
    second: U,
    done_first: bool,
}

impl<T, U> Chain<T, U> {
    pub fn into_inner(self) -> (T, U) {
        (self.first, self.second)
    }
    pub fn get_ref(&self) -> (&T, &U) {
        (&self.first, &self.second)
    }
}

impl<T: Read, U: Read> Read for Chain<T, U> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if !self.done_first {
            match self.first.read(buf)? {
                0 if !buf.is_empty() => self.done_first = true,
                n => return Ok(n),
            }
        }
        self.second.read(buf)
    }
}

pub struct Split<B> {
    buf: B,
    delim: u8,
}

impl<B: BufRead> Iterator for Split<B> {
    type Item = Result<Vec<u8>>;
    fn next(&mut self) -> Option<Result<Vec<u8>>> {
        let mut buf = Vec::new();
        match read_until(&mut self.buf, self.delim, &mut buf) {
            Ok(0) => None,
            Ok(_n) => {
                if buf[buf.len() - 1] == self.delim {
                    buf.pop();
                }
                Some(Ok(buf))
            }
            Err(e) => Some(Err(e)),
        }
    }
}

pub struct Lines<B> {
    buf: B,
}

impl<B: BufRead> Iterator for Lines<B> {
    type Item = Result<String>;
    fn next(&mut self) -> Option<Result<String>> {
        let mut buf = String::new();
        match self.buf.read_line(&mut buf) {
            Ok(0) => None,
            Ok(_n) => {
                if buf.ends_with('\n') {
                    buf.pop();
                    if buf.ends_with('\r') {
                        buf.pop();
                    }
                }
                Some(Ok(buf))
            }
            Err(e) => Some(Err(e)),
        }
    }
}

// ---- impls for references, boxes, slices and Vec

impl<R: Read + ?Sized> Read for &mut R {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        (**self).read(buf)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        (**self).read_to_end(buf)
    }
}

impl<R: Read + ?Sized> Read for alloc::boxed::Box<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        (**self).read(buf)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        (**self).read_to_end(buf)
    }
}

impl<W: Write + ?Sized> Write for &mut W {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        (**self).write(buf)
    }
    fn flush(&mut self) -> Result<()> {
        (**self).flush()
    }
}

impl<W: Write + ?Sized> Write for alloc::boxed::Box<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        (**self).write(buf)
    }
    fn flush(&mut self) -> Result<()> {
        (**self).flush()
    }
}

impl<B: BufRead + ?Sized> BufRead for &mut B {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        (**self).fill_buf()
    }
    fn consume(&mut self, amt: usize) {
        (**self).consume(amt)
    }
}

impl<B: BufRead + ?Sized> BufRead for alloc::boxed::Box<B> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        (**self).fill_buf()
    }
    fn consume(&mut self, amt: usize) {
        (**self).consume(amt)
    }
}

impl<S: Seek + ?Sized> Seek for &mut S {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        (**self).seek(pos)
    }
}

impl Read for &[u8] {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = if buf.len() < self.len() { buf.len() } else { self.len() };
        buf[..n].copy_from_slice(&self[..n]);
        *self = &self[n..];
        Ok(n)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        let n = self.len();
        buf.extend_from_slice(self);
        *self = &self[n..];
        Ok(n)
    }
}

impl BufRead for &[u8] {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        Ok(*self)
    }
    fn consume(&mut self, amt: usize) {
        *self = &self[amt..];
    }
}

impl Write for Vec<u8> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.extend_from_slice(buf);
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Write for &mut [u8] {
    fn write(&mut self, data: &[u8]) -> Result<usize> {
        let n = if data.len() < self.len() { data.len() } else { self.len() };
        let (a, b) = core::mem::take(self).split_at_mut(n);
        a.copy_from_slice(&data[..n]);
        *self = b;
        Ok(n)
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

// ---- empty / sink / copy

pub struct Empty;
pub struct Sink;

pub const fn empty() -> Empty {
    Empty
}
pub const fn sink() -> Sink {
    Sink
}

impl Read for Empty {
    fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
}
impl BufRead for Empty {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        Ok(&[])
    }
    fn consume(&mut self, _amt: usize) {}
}
impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

pub fn copy<R: Read + ?Sized, W: Write + ?Sized>(reader: &mut R, writer: &mut W) -> Result<u64> {
    let mut buf = [0u8; DEFAULT_BUF_SIZE];
    let mut total = 0u64;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) => {
                if e.is_interrupted() {
                    continue;
                }
                return Err(e);
            }
        };
        writer.write_all(&buf[..n])?;
        total += n as u64;
    }
}

pub fn read_to_string<R: Read>(mut reader: R) -> Result<String> {
    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;
    Ok(buf)
}
