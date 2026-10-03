//! io::Cursor over anything that is AsRef<[u8]>.

use alloc::vec::Vec;

use super::{BufRead, Error, ErrorKind, Read, Result, Seek, SeekFrom, Write};

#[derive(Debug, Default, Eq, PartialEq, Clone)]
pub struct Cursor<T> {
    inner: T,
    pos: u64,
}

impl<T> Cursor<T> {
    pub const fn new(inner: T) -> Cursor<T> {
        Cursor { pos: 0, inner }
    }
    pub fn into_inner(self) -> T {
        self.inner
    }
    pub const fn get_ref(&self) -> &T {
        &self.inner
    }
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }
    pub const fn position(&self) -> u64 {
        self.pos
    }
    pub fn set_position(&mut self, pos: u64) {
        self.pos = pos;
    }
}

impl<T: AsRef<[u8]>> Cursor<T> {
    fn remaining(&self) -> &[u8] {
        let data = self.inner.as_ref();
        let start = if self.pos < data.len() as u64 { self.pos as usize } else { data.len() };
        &data[start..]
    }
}

impl<T: AsRef<[u8]>> Seek for Cursor<T> {
    fn seek(&mut self, style: SeekFrom) -> Result<u64> {
        let (base, offset) = match style {
            SeekFrom::Start(n) => {
                self.pos = n;
                return Ok(n);
            }
            SeekFrom::End(n) => (self.inner.as_ref().len() as u64, n),
            SeekFrom::Current(n) => (self.pos, n),
        };
        let new = if offset >= 0 { base.checked_add(offset as u64) } else { base.checked_sub(offset.wrapping_neg() as u64) };
        match new {
            Some(n) => {
                self.pos = n;
                Ok(n)
            }
            None => Err(Error::const_msg(ErrorKind::InvalidInput, "invalid seek to a negative or overflowing position")),
        }
    }
}

impl<T: AsRef<[u8]>> Read for Cursor<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = {
            let rem = self.remaining();
            let n = if buf.len() < rem.len() { buf.len() } else { rem.len() };
            buf[..n].copy_from_slice(&rem[..n]);
            n
        };
        self.pos += n as u64;
        Ok(n)
    }
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        let n = buf.len();
        {
            let rem = self.remaining();
            if rem.len() < n {
                self.pos = self.inner.as_ref().len() as u64;
                return Err(Error::READ_EXACT_EOF);
            }
            buf.copy_from_slice(&rem[..n]);
        }
        self.pos += n as u64;
        Ok(())
    }
}

impl<T: AsRef<[u8]>> BufRead for Cursor<T> {
    fn fill_buf(&mut self) -> Result<&[u8]> {
        Ok(self.remaining())
    }
    fn consume(&mut self, amt: usize) {
        self.pos += amt as u64;
    }
}

fn slice_write(pos: &mut u64, slice: &mut [u8], buf: &[u8]) -> Result<usize> {
    let start = if *pos < slice.len() as u64 { *pos as usize } else { slice.len() };
    let space = slice.len() - start;
    let n = if buf.len() < space { buf.len() } else { space };
    slice[start..start + n].copy_from_slice(&buf[..n]);
    *pos += n as u64;
    Ok(n)
}

fn vec_write(pos: &mut u64, vec: &mut Vec<u8>, buf: &[u8]) -> Result<usize> {
    let p = *pos as usize;
    if p > vec.len() {
        vec.resize(p, 0);
    }
    let overlap = if vec.len() - p < buf.len() { vec.len() - p } else { buf.len() };
    vec[p..p + overlap].copy_from_slice(&buf[..overlap]);
    vec.extend_from_slice(&buf[overlap..]);
    *pos += buf.len() as u64;
    Ok(buf.len())
}

impl Write for Cursor<&mut [u8]> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        slice_write(&mut self.pos, self.inner, buf)
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Write for Cursor<&mut Vec<u8>> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        vec_write(&mut self.pos, self.inner, buf)
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Write for Cursor<Vec<u8>> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        vec_write(&mut self.pos, &mut self.inner, buf)
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
