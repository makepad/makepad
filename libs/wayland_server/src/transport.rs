//! Per-connection byte and fd transport: `recvmsg`/`sendmsg` with `SCM_RIGHTS`.
use crate::sys::*;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;

/// Most fds one `sendmsg`/`recvmsg` carries.
pub const MAX_FDS_PER_MSG: usize = 28;
const CHUNK: usize = 4096;
/// Bytes buffered for a client that does not read before it is disconnected.
pub const MAX_OUT_BYTES: usize = 4 << 20;
/// Received fds that no message has consumed yet before the client is disconnected.
pub const MAX_IN_FDS: usize = 256;
/// Bytes read from one client per wakeup, for fairness.
const READ_BUDGET: usize = 256 << 10;

pub struct Transport {
    pub stream: UnixStream,
    pub in_buf: Vec<u8>,
    pub in_fds: VecDeque<OwnedFd>,
    pub out_buf: Vec<u8>,
    /// (offset in `out_buf` of the message that carries it, fd)
    pub out_fds: VecDeque<(usize, OwnedFd)>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Fill { More, Closed, Budget }

impl Transport {
    pub fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self { stream, in_buf: Vec::new(), in_fds: VecDeque::new(), out_buf: Vec::new(), out_fds: VecDeque::new() })
    }
    pub fn peer_cred(&self) -> (i32, u32, u32) {
        let mut c = ucred { pid: 0, uid: 0, gid: 0 };
        let mut len = std::mem::size_of::<ucred>() as u32;
        unsafe { getsockopt(self.stream.as_raw_fd(), SOL_SOCKET, SO_PEERCRED, &mut c as *mut _ as *mut c_void, &mut len) };
        (c.pid, c.uid, c.gid)
    }
    /// Reads until the socket would block, the peer closed, or the budget is used.
    /// Err means a protocol-level failure (control data truncated, fd flood) or an I/O error.
    pub fn fill(&mut self) -> io::Result<Fill> {
        let mut total = 0;
        let mut data = [0u8; CHUNK];
        let mut ctl = [0u64; (28 * 4 + 16 + 16) / 8 + 1];
        loop {
            let mut iov = iovec { base: data.as_mut_ptr() as *mut c_void, len: CHUNK };
            let mut msg = msghdr {
                name: std::ptr::null_mut(), namelen: 0, iov: &mut iov, iovlen: 1,
                control: ctl.as_mut_ptr() as *mut c_void, controllen: std::mem::size_of_val(&ctl), flags: 0,
            };
            let n = unsafe { recvmsg(self.stream.as_raw_fd(), &mut msg, MSG_DONTWAIT | MSG_CMSG_CLOEXEC) };
            if n < 0 {
                let e = io::Error::last_os_error();
                match e.raw_os_error() {
                    Some(EAGAIN) => return Ok(Fill::More),
                    Some(EINTR) => continue,
                    _ => return Err(e),
                }
            }
            // Own every received fd first so none leak on an error path.
            let mut p = msg.control as *const u8;
            let end = unsafe { p.add(msg.controllen) };
            let hdr = std::mem::size_of::<cmsghdr>();
            while msg.controllen >= hdr && (end as usize) - (p as usize) >= hdr {
                let h = unsafe { std::ptr::read_unaligned(p as *const cmsghdr) };
                if h.len < hdr || (p as usize) + h.len > end as usize { break; }
                if h.level == SOL_SOCKET && h.kind == SCM_RIGHTS {
                    let count = (h.len - cmsg_len(0)) / 4;
                    for i in 0..count {
                        let fd = unsafe { std::ptr::read_unaligned(p.add(cmsg_len(0) + i * 4) as *const i32) };
                        self.in_fds.push_back(unsafe { OwnedFd::from_raw_fd(fd) });
                    }
                }
                p = unsafe { p.add(cmsg_align(h.len)) };
            }
            if msg.flags & MSG_CTRUNC != 0 { return Err(io::Error::new(io::ErrorKind::InvalidData, "control data truncated")); }
            if self.in_fds.len() > MAX_IN_FDS { return Err(io::Error::new(io::ErrorKind::InvalidData, "too many unconsumed fds")); }
            if n == 0 { return Ok(Fill::Closed); }
            self.in_buf.extend_from_slice(&data[..n as usize]);
            total += n as usize;
            if total >= READ_BUDGET { return Ok(Fill::Budget); }
        }
    }
    /// Queues an encoded message with the fds that travel with it.
    pub fn queue(&mut self, bytes: &[u8], fds: Vec<OwnedFd>) -> bool {
        let at = self.out_buf.len();
        self.out_buf.extend_from_slice(bytes);
        for fd in fds { self.out_fds.push_back((at, fd)); }
        self.out_buf.len() <= MAX_OUT_BYTES
    }
    /// Sends as much as the socket takes. Returns true when everything is flushed.
    pub fn flush(&mut self) -> io::Result<bool> {
        while !self.out_buf.is_empty() {
            let mut len = self.out_buf.len().min(CHUNK);
            // Fds whose message starts inside this chunk travel with it, at most 28.
            let mut nfds = self.out_fds.iter().take_while(|(o, _)| *o < len).count();
            if nfds > MAX_FDS_PER_MSG {
                nfds = MAX_FDS_PER_MSG;
                len = self.out_fds[MAX_FDS_PER_MSG].0.max(1);
                if len > self.out_buf.len() { len = self.out_buf.len(); }
            }
            let raw: Vec<i32> = self.out_fds.iter().take(nfds).map(|(_, f)| f.as_raw_fd()).collect();
            let mut ctl = [0u64; (28 * 4 + 16 + 16) / 8 + 1];
            let mut iov = iovec { base: self.out_buf.as_ptr() as *mut c_void, len };
            let mut msg = msghdr { name: std::ptr::null_mut(), namelen: 0, iov: &mut iov, iovlen: 1, control: std::ptr::null_mut(), controllen: 0, flags: 0 };
            if nfds > 0 {
                let h = cmsghdr { len: cmsg_len(nfds * 4), level: SOL_SOCKET, kind: SCM_RIGHTS };
                let base = ctl.as_mut_ptr() as *mut u8;
                unsafe {
                    std::ptr::write(base as *mut cmsghdr, h);
                    std::ptr::copy_nonoverlapping(raw.as_ptr() as *const u8, base.add(cmsg_len(0)), nfds * 4);
                }
                msg.control = ctl.as_mut_ptr() as *mut c_void;
                msg.controllen = cmsg_space(nfds * 4);
            }
            let n = unsafe { sendmsg(self.stream.as_raw_fd(), &msg, MSG_DONTWAIT | MSG_NOSIGNAL) };
            if n < 0 {
                let e = io::Error::last_os_error();
                match e.raw_os_error() {
                    Some(EAGAIN) => return Ok(false),
                    Some(EINTR) => continue,
                    _ => return Err(e),
                }
            }
            let n = n as usize;
            // The fds went out with the first byte; close our copies.
            self.out_fds.drain(..nfds);
            self.out_buf.drain(..n);
            for (o, _) in self.out_fds.iter_mut() { *o = o.saturating_sub(n); }
        }
        Ok(true)
    }
}
