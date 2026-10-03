//! BufReader, BufWriter, LineWriter.

use alloc::vec::Vec;
use core::fmt;

use super::{BufRead, Error, ErrorKind, Read, Result, Seek, SeekFrom, Write, DEFAULT_BUF_SIZE};

pub struct BufReader<R> {
    buf: Vec<u8>, // len == capacity of the buffer
    pos: usize,
    filled: usize,
    inner: R,
}

impl<R: Read> BufReader<R> {
    pub fn new(inner: R) -> BufReader<R> {
        BufReader::with_capacity(DEFAULT_BUF_SIZE, inner)
    }
    pub fn with_capacity(capacity: usize, inner: R) -> BufReader<R> {
        let mut buf = Vec::with_capacity(capacity);
        buf.resize(capacity, 0);
        BufReader { buf, pos: 0, filled: 0, inner }
    }
}

impl<R> BufReader<R> {
    pub fn get_ref(&self) -> &R {
        &self.inner
    }
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }
    pub fn buffer(&self) -> &[u8] {
        &self.buf[self.pos..self.filled]
    }
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }
    pub fn into_inner(self) -> R {
        self.inner
    }
    fn discard_buffer(&mut self) {
        self.pos = 0;
        self.filled = 0;
    }
}

impl<R: Read> Read for BufReader<R> {
    fn read(&mut self, out: &mut [u8]) -> Result<usize> {
        // large reads bypass an empty buffer
        if self.pos == self.filled && out.len() >= self.buf.len() {
            self.discard_buffer();
            return self.inner.read(out);
        }
        let n = {
            let rem = self.fill_buf()?;
            let n = if out.len() < rem.len() { out.len() } else { rem.len() };
            out[..n].copy_from_slice(&rem[..n]);
            n
        };
        self.consume(n);
        Ok(n)
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> Result<usize> {
        let n = self.filled - self.pos;
        buf.extend_from_slice(&self.buf[self.pos..self.filled]);
        self.discard_buffer();
        Ok(n + self.inner.read_to_end(buf)?)
    }
}

impl<R: Read> BufRead for BufReader<R> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        if self.pos >= self.filled {
            let n = self.inner.read(&mut self.buf)?;
            self.pos = 0;
            self.filled = n;
        }
        Ok(&self.buf[self.pos..self.filled])
    }
    fn consume(&mut self, amt: usize) {
        self.pos = if self.pos + amt < self.filled { self.pos + amt } else { self.filled };
    }
}

impl<R: Seek> Seek for BufReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let result;
        if let SeekFrom::Current(n) = pos {
            let remainder = (self.filled - self.pos) as i64;
            if let Some(offset) = n.checked_sub(remainder) {
                result = self.inner.seek(SeekFrom::Current(offset))?;
            } else {
                self.inner.seek(SeekFrom::Current(-remainder))?;
                self.discard_buffer();
                result = self.inner.seek(SeekFrom::Current(n))?;
            }
        } else {
            result = self.inner.seek(pos)?;
        }
        self.discard_buffer();
        Ok(result)
    }
}

impl<R: fmt::Debug> fmt::Debug for BufReader<R> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("BufReader")
            .field("reader", &self.inner)
            .field("buffer", &format_args!("{}/{}", self.filled - self.pos, self.buf.len()))
            .finish()
    }
}

pub struct BufWriter<W: Write> {
    buf: Vec<u8>,
    cap: usize,
    panicked: bool,
    inner: Option<W>,
}

#[derive(Debug)]
pub struct IntoInnerError<W>(W, Error);

impl<W> IntoInnerError<W> {
    pub fn error(&self) -> &Error {
        &self.1
    }
    pub fn into_inner(self) -> W {
        self.0
    }
    pub fn into_error(self) -> Error {
        self.1
    }
}

impl<W> fmt::Display for IntoInnerError<W> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.1, f)
    }
}

impl<W: Write> BufWriter<W> {
    pub fn new(inner: W) -> BufWriter<W> {
        BufWriter::with_capacity(DEFAULT_BUF_SIZE, inner)
    }
    pub fn with_capacity(capacity: usize, inner: W) -> BufWriter<W> {
        BufWriter { buf: Vec::with_capacity(capacity), cap: capacity, panicked: false, inner: Some(inner) }
    }
    fn inner_mut(&mut self) -> &mut W {
        match self.inner.as_mut() {
            Some(w) => w,
            None => panic!("BufWriter used after into_inner"),
        }
    }
    fn flush_buf(&mut self) -> Result<()> {
        let mut written = 0;
        let mut ret = Ok(());
        while written < self.buf.len() {
            self.panicked = true;
            let r = match self.inner.as_mut() {
                Some(w) => w.write(&self.buf[written..]),
                None => Ok(0),
            };
            self.panicked = false;
            match r {
                Ok(0) => {
                    ret = Err(Error::const_msg(ErrorKind::WriteZero, "failed to write the buffered data"));
                    break;
                }
                Ok(n) => written += n,
                Err(e) => {
                    if !e.is_interrupted() {
                        ret = Err(e);
                        break;
                    }
                }
            }
        }
        self.buf.drain(..written);
        ret
    }
    pub fn get_ref(&self) -> &W {
        match self.inner.as_ref() {
            Some(w) => w,
            None => panic!("BufWriter used after into_inner"),
        }
    }
    pub fn get_mut(&mut self) -> &mut W {
        self.inner_mut()
    }
    pub fn buffer(&self) -> &[u8] {
        &self.buf
    }
    pub fn capacity(&self) -> usize {
        self.cap
    }
    pub fn into_inner(mut self) -> core::result::Result<W, IntoInnerError<BufWriter<W>>> {
        match self.flush_buf() {
            Err(e) => Err(IntoInnerError(self, e)),
            Ok(()) => Ok(self.inner.take().unwrap()),
        }
    }
}

impl<W: Write> Write for BufWriter<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        if self.buf.len() + buf.len() > self.cap {
            self.flush_buf()?;
        }
        if buf.len() >= self.cap {
            self.panicked = true;
            let r = self.inner_mut().write(buf);
            self.panicked = false;
            r
        } else {
            self.buf.extend_from_slice(buf);
            Ok(buf.len())
        }
    }
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        if self.buf.len() + buf.len() > self.cap {
            self.flush_buf()?;
        }
        if buf.len() >= self.cap {
            self.panicked = true;
            let r = self.inner_mut().write_all(buf);
            self.panicked = false;
            r
        } else {
            self.buf.extend_from_slice(buf);
            Ok(())
        }
    }
    fn flush(&mut self) -> Result<()> {
        self.flush_buf()?;
        self.inner_mut().flush()
    }
}

impl<W: Write + Seek> Seek for BufWriter<W> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.flush_buf()?;
        self.inner_mut().seek(pos)
    }
}

impl<W: Write> Drop for BufWriter<W> {
    fn drop(&mut self) {
        if self.inner.is_some() && !self.panicked {
            let _ = self.flush_buf();
        }
    }
}

impl<W: Write + fmt::Debug> fmt::Debug for BufWriter<W> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("BufWriter")
            .field("writer", self.get_ref())
            .field("buffer", &format_args!("{}/{}", self.buf.len(), self.cap))
            .finish()
    }
}

/// Buffers until a newline, then writes everything up to the last newline.
pub struct LineWriter<W: Write> {
    inner: BufWriter<W>,
}

impl<W: Write> LineWriter<W> {
    pub fn new(inner: W) -> LineWriter<W> {
        LineWriter { inner: BufWriter::with_capacity(1024, inner) }
    }
    pub fn with_capacity(capacity: usize, inner: W) -> LineWriter<W> {
        LineWriter { inner: BufWriter::with_capacity(capacity, inner) }
    }
    pub fn get_ref(&self) -> &W {
        self.inner.get_ref()
    }
    pub fn get_mut(&mut self) -> &mut W {
        self.inner.get_mut()
    }
}

impl<W: Write> Write for LineWriter<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let mut last_nl = None;
        let mut i = buf.len();
        while i > 0 {
            i -= 1;
            if buf[i] == b'\n' {
                last_nl = Some(i);
                break;
            }
        }
        match last_nl {
            None => {
                // a previously buffered complete line is flushed first
                if self.inner.buffer().last() == Some(&b'\n') {
                    self.inner.flush_buf()?;
                }
                self.inner.write(buf)
            }
            Some(nl) => {
                self.inner.flush_buf()?;
                let lines = &buf[..nl + 1];
                let n = self.inner.inner_mut().write(lines)?;
                if n == 0 {
                    return Ok(0);
                }
                if n < lines.len() {
                    return Ok(n);
                }
                let tail = &buf[n..];
                let m = if tail.len() < self.inner.capacity() { self.inner.write(tail)? } else { 0 };
                Ok(n + m)
            }
        }
    }
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
    fn flush(&mut self) -> Result<()> {
        self.inner.flush()
    }
}
