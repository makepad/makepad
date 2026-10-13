//! Framing: `len u32 LE (excl. itself) | chan u16 LE | kind u8 | flags u8 | payload`.
//! The largest frame is 1 MiB; a longer message is split by the writer into frames carrying
//! [`FLAG_MORE`] and put back together by the reader, so a service always sees whole messages.

use std::io::{self, Read, Write};

/// Largest `len` a frame may announce (the four header bytes after `len` included).
pub const MAX_FRAME: usize = 1 << 20;
/// chan + kind + flags
pub const HEADER: usize = 4;
pub const MAX_PAYLOAD: usize = MAX_FRAME - HEADER;
/// DATA continues in the next DATA frame of the same channel.
pub const FLAG_MORE: u8 = 1;

pub mod kind {
    /// `service \n args` (utf8)
    pub const OPEN: u8 = 1;
    /// the service's caps (`name/version`, utf8)
    pub const OK: u8 = 2;
    pub const DATA: u8 = 3;
    /// `bytes u32 LE`
    pub const CREDIT: u8 = 4;
    /// `code u16 LE | reason utf8`
    pub const CLOSE: u8 = 5;
    /// `mono_ns u64 LE`
    pub const PING: u8 = 6;
    pub const PONG: u8 = 7;
    /// Reserved: a payload with passed file descriptors / ports (local transports; not in T0).
    pub const HANDLE: u8 = 8;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub chan: u16,
    pub kind: u8,
    pub flags: u8,
    pub payload: Vec<u8>,
}

pub fn write_frame(w: &mut dyn Write, chan: u16, kind: u8, flags: u8, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_PAYLOAD {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame payload above the 1 MiB limit"));
    }
    let mut head = [0u8; 8];
    head[..4].copy_from_slice(&((payload.len() + HEADER) as u32).to_le_bytes());
    head[4..6].copy_from_slice(&chan.to_le_bytes());
    head[6] = kind;
    head[7] = flags;
    w.write_all(&head)?;
    w.write_all(payload)
}

/// Reads frames from a byte stream. A read that times out (`WouldBlock` / `TimedOut`, as on the TLS
/// carrier) is retried, so a frame may arrive in pieces with pauses between them; `cancel` ends
/// the wait, as does `deadline`.
pub struct FrameReader {
    r: Box<dyn Read + Send>,
    buf: Vec<u8>,
    start: usize,
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub deadline: Option<std::time::Instant>,
}

impl FrameReader {
    pub fn new(r: Box<dyn Read + Send>) -> Self {
        Self { r, buf: Vec::new(), start: 0, cancel: None, deadline: None }
    }

    fn take(&mut self) -> io::Result<Option<Frame>> {
        let avail = &self.buf[self.start..];
        if avail.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes([avail[0], avail[1], avail[2], avail[3]]) as usize;
        if len > MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("peer sent a frame of {len} bytes; the limit is {MAX_FRAME}")));
        }
        if len < HEADER {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("peer sent a frame of {len} bytes, shorter than its header")));
        }
        if avail.len() < 4 + len {
            return Ok(None);
        }
        let f = Frame { chan: u16::from_le_bytes([avail[4], avail[5]]), kind: avail[6], flags: avail[7], payload: avail[8..4 + len].to_vec() };
        self.start += 4 + len;
        if self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        }
        Ok(Some(f))
    }

    pub fn next(&mut self) -> io::Result<Frame> {
        loop {
            if let Some(f) = self.take()? {
                return Ok(f);
            }
            if self.start > 0 {
                self.buf.drain(..self.start);
                self.start = 0;
            }
            if self.cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
                return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "connection closed"));
            }
            if self.deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out waiting for the peer"));
            }
            let mut chunk = [0u8; 64 << 10];
            match self.r.read(&mut chunk) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed by the peer")),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {}
                Err(e) => return Err(e),
            }
        }
    }
}
