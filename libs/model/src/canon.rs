use crate::{Error, Result};

pub(crate) struct Writer { pub bytes: Vec<u8>, limit: usize }
impl Writer {
    pub fn new(limit: usize) -> Self { Self { bytes: Vec::new(), limit } }
    pub fn raw(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) { return Err(Error::Budget("serialized bytes")); }
        self.bytes.extend_from_slice(bytes); Ok(())
    }
    pub fn u8(&mut self, v: u8) -> Result<()> { self.raw(&[v]) }
    pub fn u32(&mut self, v: u32) -> Result<()> { self.raw(&v.to_le_bytes()) }
    pub fn u64(&mut self, v: u64) -> Result<()> { self.raw(&v.to_le_bytes()) }
    pub fn count(&mut self, v: usize) -> Result<()> { self.u32(v.try_into().map_err(|_| Error::Budget("count"))?) }
    pub fn f64(&mut self, v: f64) -> Result<()> {
        if !v.is_finite() { return Err(Error::Invalid("non-finite number")); }
        self.u64(if v == 0.0 { 0 } else { v.to_bits() })
    }
    pub fn blob(&mut self, v: &[u8]) -> Result<()> { self.count(v.len())?; self.raw(v) }
    pub fn string(&mut self, v: &str) -> Result<()> { self.blob(v.as_bytes()) }
}

pub(crate) struct Reader<'a> { bytes: &'a [u8], pos: usize }
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self { Self { bytes, pos: 0 } }
    pub fn raw(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.bytes.len().saturating_sub(self.pos) { return Err(Error::Corrupt("truncated source")); }
        let p = self.pos; self.pos += n; Ok(&self.bytes[p..self.pos])
    }
    pub fn u8(&mut self) -> Result<u8> { Ok(self.raw(1)?[0]) }
    pub fn u32(&mut self) -> Result<u32> { Ok(u32::from_le_bytes(self.raw(4)?.try_into().unwrap())) }
    pub fn u64(&mut self) -> Result<u64> { Ok(u64::from_le_bytes(self.raw(8)?.try_into().unwrap())) }
    pub fn count(&mut self, max: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n > max || n > self.bytes.len().saturating_sub(self.pos) { return Err(Error::Budget("source count")); }
        Ok(n)
    }
    pub fn f64(&mut self) -> Result<f64> {
        let bits = self.u64()?; let v = f64::from_bits(bits);
        if !v.is_finite() || bits == (-0.0f64).to_bits() { return Err(Error::Corrupt("noncanonical number")); }
        Ok(v)
    }
    pub fn blob(&mut self, max: usize) -> Result<&'a [u8]> { let n = self.count(max)?; self.raw(n) }
    pub fn string(&mut self, max: usize) -> Result<String> {
        String::from_utf8(self.blob(max)?.to_vec()).map_err(|_| Error::Corrupt("UTF-8"))
    }
    pub fn end(&self) -> Result<()> {
        if self.pos == self.bytes.len() { Ok(()) } else { Err(Error::Corrupt("trailing bytes")) }
    }
}
