//! Little-endian binary reader/writer used by the `region.search` and
//! `region.graph` artifact formats. Builders and readers live in the same
//! crate so the formats cannot drift.

#[derive(Debug, Clone, PartialEq)]
pub enum NavFmtError {
    UnexpectedEof,
    BadMagic,
    BadVersion(u32),
    BadUtf8,
    Corrupt(&'static str),
}

impl std::fmt::Display for NavFmtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NavFmtError::UnexpectedEof => write!(f, "unexpected end of file"),
            NavFmtError::BadMagic => write!(f, "bad file magic"),
            NavFmtError::BadVersion(v) => write!(f, "unsupported format version {}", v),
            NavFmtError::BadUtf8 => write!(f, "invalid utf8 string"),
            NavFmtError::Corrupt(what) => write!(f, "corrupt file: {}", what),
        }
    }
}

impl std::error::Error for NavFmtError {}

#[derive(Default)]
pub struct ByteWriter {
    pub buf: Vec<u8>,
}

impl ByteWriter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }
    /// u32 length-prefixed utf8 string.
    pub fn str32(&mut self, v: &str) {
        self.u32(v.len() as u32);
        self.buf.extend_from_slice(v.as_bytes());
    }
}

pub struct ByteReader<'a> {
    data: &'a [u8],
    pub pos: usize,
}

impl<'a> ByteReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], NavFmtError> {
        if self.remaining() < n {
            return Err(NavFmtError::UnexpectedEof);
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }
    pub fn u8(&mut self) -> Result<u8, NavFmtError> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, NavFmtError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, NavFmtError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, NavFmtError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32, NavFmtError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn f64(&mut self) -> Result<f64, NavFmtError> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], NavFmtError> {
        self.take(n)
    }
    pub fn str32(&mut self) -> Result<String, NavFmtError> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| NavFmtError::BadUtf8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut w = ByteWriter::new();
        w.u8(7);
        w.u16(65535);
        w.u32(123_456_789);
        w.u64(u64::MAX - 3);
        w.f32(1.5);
        w.f64(-2.25);
        w.str32("hëllo wörld");
        let mut r = ByteReader::new(&w.buf);
        assert_eq!(r.u8().unwrap(), 7);
        assert_eq!(r.u16().unwrap(), 65535);
        assert_eq!(r.u32().unwrap(), 123_456_789);
        assert_eq!(r.u64().unwrap(), u64::MAX - 3);
        assert_eq!(r.f32().unwrap(), 1.5);
        assert_eq!(r.f64().unwrap(), -2.25);
        assert_eq!(r.str32().unwrap(), "hëllo wörld");
        assert_eq!(r.remaining(), 0);
        assert!(r.u8().is_err());
    }
}
