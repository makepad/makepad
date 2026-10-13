//! Wire codec: 8-byte header (object id, size<<16 | opcode) and the argument encodings.
use crate::schema::ArgType;
use std::collections::VecDeque;
use std::os::fd::OwnedFd;

pub const MAX_MESSAGE: usize = 4096;
pub const HEADER: usize = 8;

#[derive(Debug)]
pub enum Value {
    Int(i32),
    Uint(u32),
    /// 24.8 fixed point, raw.
    Fixed(i32),
    Str(Option<String>),
    Array(Vec<u8>),
    Fd(OwnedFd),
    /// Object id; 0 is null.
    Object(u32),
    NewId(u32),
    NewIdDyn { interface: String, version: u32, id: u32 },
}
impl Value {
    pub fn fixed(v: f64) -> Value { Value::Fixed((v * 256.0) as i32) }
    pub fn as_uint(&self) -> u32 { match self { Value::Uint(v) => *v, Value::Int(v) => *v as u32, Value::Object(v) | Value::NewId(v) => *v, _ => 0 } }
    pub fn as_int(&self) -> i32 { match self { Value::Int(v) => *v, Value::Uint(v) => *v as i32, _ => 0 } }
    pub fn as_f64(&self) -> f64 { match self { Value::Fixed(v) => *v as f64 / 256.0, _ => 0.0 } }
    pub fn as_bytes(&self) -> &[u8] { match self { Value::Array(a) => a, _ => &[] } }
    pub fn as_str(&self) -> &str { match self { Value::Str(Some(s)) => s, _ => "" } }
}

#[derive(Debug, PartialEq, Eq)]
pub enum WireError {
    BadSize,
    Truncated,
    Trailing,
    BadString,
    NullString,
    MissingFd,
    TooLong,
    TypeMismatch,
}
impl WireError {
    pub fn text(&self) -> &'static str {
        match self {
            WireError::BadSize => "bad message size",
            WireError::Truncated => "message too short for its arguments",
            WireError::Trailing => "message longer than its arguments",
            WireError::BadString => "string is not valid UTF-8 or lacks its terminator",
            WireError::NullString => "null string for a non-nullable argument",
            WireError::MissingFd => "message needs a file descriptor that was not sent",
            WireError::TooLong => "encoded message exceeds 4096 bytes",
            WireError::TypeMismatch => "argument does not match the message signature",
        }
    }
}

/// Reads a header from the start of `buf`: (object id, opcode, total size). `None` if fewer than 8 bytes.
pub fn peek_header(buf: &[u8]) -> Option<(u32, u16, usize)> {
    if buf.len() < HEADER { return None; }
    let id = u32::from_ne_bytes(buf[0..4].try_into().unwrap());
    let word = u32::from_ne_bytes(buf[4..8].try_into().unwrap());
    Some((id, (word & 0xffff) as u16, (word >> 16) as usize))
}

struct Reader<'a> { buf: &'a [u8], at: usize }
impl<'a> Reader<'a> {
    fn word(&mut self) -> Result<u32, WireError> {
        let b = self.buf.get(self.at..self.at + 4).ok_or(WireError::Truncated)?;
        self.at += 4;
        Ok(u32::from_ne_bytes(b.try_into().unwrap()))
    }
    fn bytes(&mut self, len: usize) -> Result<&'a [u8], WireError> {
        let padded = len.checked_add(3).ok_or(WireError::Truncated)? & !3;
        let b = self.buf.get(self.at..self.at.checked_add(padded).ok_or(WireError::Truncated)?).ok_or(WireError::Truncated)?;
        self.at += padded;
        Ok(&b[..len])
    }
    fn string(&mut self, nullable: bool) -> Result<Option<String>, WireError> {
        let len = self.word()? as usize;
        if len == 0 { return if nullable { Ok(None) } else { Err(WireError::NullString) }; }
        let b = self.bytes(len)?;
        let (last, text) = b.split_last().unwrap();
        if *last != 0 { return Err(WireError::BadString); }
        String::from_utf8(text.to_vec()).map(Some).map_err(|_| WireError::BadString)
    }
}

/// Decodes the argument block (everything after the header) according to `sig`.
/// File descriptors are taken in order from `fds`.
pub fn decode_args(body: &[u8], sig: &[ArgType], fds: &mut VecDeque<OwnedFd>) -> Result<Vec<Value>, WireError> {
    let mut r = Reader { buf: body, at: 0 };
    let mut out = Vec::with_capacity(sig.len());
    for t in sig {
        out.push(match t {
            ArgType::Int => Value::Int(r.word()? as i32),
            ArgType::Uint => Value::Uint(r.word()?),
            ArgType::Fixed => Value::Fixed(r.word()? as i32),
            ArgType::Str => Value::Str(r.string(false)?),
            ArgType::StrOpt => Value::Str(r.string(true)?),
            ArgType::Array => { let len = r.word()? as usize; Value::Array(r.bytes(len)?.to_vec()) }
            ArgType::Fd => Value::Fd(fds.pop_front().ok_or(WireError::MissingFd)?),
            ArgType::Object(_) | ArgType::ObjectOpt(_) => Value::Object(r.word()?),
            ArgType::NewId(_) => Value::NewId(r.word()?),
            ArgType::NewIdDyn => {
                let interface = r.string(false)?.unwrap();
                let version = r.word()?;
                Value::NewIdDyn { interface, version, id: r.word()? }
            }
        });
    }
    if r.at != body.len() { return Err(WireError::Trailing); }
    Ok(out)
}

fn put_bytes(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(data);
    while out.len() % 4 != 0 { out.push(0); }
}

/// Encodes one message. File descriptors are moved into `fds` in argument order.
pub fn encode(object: u32, opcode: u16, sig: &[ArgType], args: Vec<Value>, fds: &mut Vec<OwnedFd>) -> Result<Vec<u8>, WireError> {
    if sig.len() != args.len() { return Err(WireError::TypeMismatch); }
    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(&object.to_ne_bytes());
    out.extend_from_slice(&0u32.to_ne_bytes());
    for (t, v) in sig.iter().zip(args) {
        match (t, v) {
            (ArgType::Int, Value::Int(v)) => out.extend_from_slice(&v.to_ne_bytes()),
            (ArgType::Uint, Value::Uint(v)) => out.extend_from_slice(&v.to_ne_bytes()),
            (ArgType::Fixed, Value::Fixed(v)) => out.extend_from_slice(&v.to_ne_bytes()),
            (ArgType::Str | ArgType::StrOpt, Value::Str(s)) => match s {
                Some(s) => {
                    out.extend_from_slice(&(s.len() as u32 + 1).to_ne_bytes());
                    out.extend_from_slice(s.as_bytes());
                    out.push(0);
                    while out.len() % 4 != 0 { out.push(0); }
                }
                None if *t == ArgType::StrOpt => out.extend_from_slice(&0u32.to_ne_bytes()),
                None => return Err(WireError::NullString),
            },
            (ArgType::Array, Value::Array(a)) => {
                out.extend_from_slice(&(a.len() as u32).to_ne_bytes());
                put_bytes(&mut out, &a);
            }
            (ArgType::Fd, Value::Fd(fd)) => fds.push(fd),
            (ArgType::Object(_) | ArgType::ObjectOpt(_), Value::Object(v)) => out.extend_from_slice(&v.to_ne_bytes()),
            (ArgType::NewId(_), Value::NewId(v)) => out.extend_from_slice(&v.to_ne_bytes()),
            (ArgType::NewIdDyn, Value::NewIdDyn { interface, version, id }) => {
                out.extend_from_slice(&(interface.len() as u32 + 1).to_ne_bytes());
                out.extend_from_slice(interface.as_bytes());
                out.push(0);
                while out.len() % 4 != 0 { out.push(0); }
                out.extend_from_slice(&version.to_ne_bytes());
                out.extend_from_slice(&id.to_ne_bytes());
            }
            _ => return Err(WireError::TypeMismatch),
        }
    }
    if out.len() > MAX_MESSAGE { return Err(WireError::TooLong); }
    let word = ((out.len() as u32) << 16) | opcode as u32;
    out[4..8].copy_from_slice(&word.to_ne_bytes());
    Ok(out)
}
