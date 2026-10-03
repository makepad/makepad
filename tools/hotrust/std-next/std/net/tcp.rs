//! TcpStream / TcpListener over BSD sockets.

use core::ffi::{c_int, c_void};
use core::fmt;
use core::time::Duration;

use super::{Shutdown, SocketAddr, ToSocketAddrs};
use crate::io::{self, Read, Write};
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::sys;
use crate::sys::net as sn;

/// A socket fd with the helpers TcpStream/TcpListener/UnixStream share.
pub(crate) struct Socket {
    fd: OwnedFd,
}

fn cvt(r: c_int) -> io::Result<c_int> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

impl Socket {
    pub(crate) fn new(family: c_int, ty: c_int) -> io::Result<Socket> {
        let fd = cvt(unsafe { sn::socket(family, ty, 0) })?;
        let s = Socket { fd: unsafe { OwnedFd::from_raw_fd(fd) } };
        unsafe {
            sys::fcntl(fd, sys::F_SETFD, sys::FD_CLOEXEC);
        }
        if cfg!(target_os = "macos") {
            s.set_int(sn::SOL_SOCKET, sn::SO_NOSIGPIPE, 1)?;
        }
        Ok(s)
    }
    pub(crate) fn from_fd(fd: c_int) -> Socket {
        Socket { fd: unsafe { OwnedFd::from_raw_fd(fd) } }
    }
    pub(crate) fn raw(&self) -> c_int {
        self.fd.as_raw_fd()
    }
    pub(crate) fn owned(&self) -> &OwnedFd {
        &self.fd
    }
    pub(crate) fn into_owned(self) -> OwnedFd {
        self.fd
    }
    pub(crate) fn set_int(&self, level: c_int, name: c_int, v: c_int) -> io::Result<()> {
        cvt(unsafe { sn::setsockopt(self.raw(), level, name, &v as *const c_int as *const c_void, 4) })?;
        Ok(())
    }
    pub(crate) fn get_int(&self, level: c_int, name: c_int) -> io::Result<c_int> {
        let mut v: c_int = 0;
        let mut len: sn::socklen_t = 4;
        cvt(unsafe { sn::getsockopt(self.raw(), level, name, &mut v as *mut c_int as *mut c_void, &mut len as *mut sn::socklen_t) })?;
        Ok(v)
    }
    pub(crate) fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let r = unsafe { sn::recv(self.raw(), buf.as_mut_ptr() as *mut c_void, buf.len(), 0) };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(r as usize)
        }
    }
    pub(crate) fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        let r = unsafe { sn::recv(self.raw(), buf.as_mut_ptr() as *mut c_void, buf.len(), sn::MSG_PEEK) };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(r as usize)
        }
    }
    pub(crate) fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let r = unsafe { sn::send(self.raw(), buf.as_ptr() as *const c_void, buf.len(), sn::MSG_NOSIGNAL) };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(r as usize)
        }
    }
    pub(crate) fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        let h = match how {
            Shutdown::Read => sn::SHUT_RD,
            Shutdown::Write => sn::SHUT_WR,
            Shutdown::Both => sn::SHUT_RDWR,
        };
        cvt(unsafe { sn::shutdown(self.raw(), h) })?;
        Ok(())
    }
    pub(crate) fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        let mut v: c_int = nonblocking as c_int;
        cvt(unsafe { sn::ioctl(self.raw(), sn::FIONBIO, &mut v as *mut c_int) })?;
        Ok(())
    }
    pub(crate) fn set_timeout(&self, dur: Option<Duration>, kind: c_int) -> io::Result<()> {
        let tv = match dur {
            Some(d) => {
                if d.is_zero() {
                    return Err(io::Error::ZERO_TIMEOUT);
                }
                let secs = if d.as_secs() > i64::MAX as u64 { i64::MAX } else { d.as_secs() as i64 };
                let mut usec = d.subsec_micros() as i64;
                if secs == 0 && usec == 0 {
                    usec = 1;
                }
                sn::TimevalC { tv_sec: secs, tv_usec: usec }
            }
            None => sn::TimevalC { tv_sec: 0, tv_usec: 0 },
        };
        cvt(unsafe { sn::setsockopt(self.raw(), sn::SOL_SOCKET, kind, &tv as *const sn::TimevalC as *const c_void, 16) })?;
        Ok(())
    }
    pub(crate) fn timeout(&self, kind: c_int) -> io::Result<Option<Duration>> {
        let mut tv = sn::TimevalC { tv_sec: 0, tv_usec: 0 };
        let mut len: sn::socklen_t = 16;
        cvt(unsafe { sn::getsockopt(self.raw(), sn::SOL_SOCKET, kind, &mut tv as *mut sn::TimevalC as *mut c_void, &mut len as *mut sn::socklen_t) })?;
        let usec = (tv.tv_usec & 0xffff_ffff) as u32;
        if tv.tv_sec == 0 && usec == 0 {
            Ok(None)
        } else {
            Ok(Some(Duration::new(tv.tv_sec as u64, usec * 1000)))
        }
    }
    pub(crate) fn take_error(&self) -> io::Result<Option<io::Error>> {
        let e = self.get_int(sn::SOL_SOCKET, sn::SO_ERROR)?;
        if e == 0 {
            Ok(None)
        } else {
            Ok(Some(io::Error::from_raw_os_error(e)))
        }
    }
    pub(crate) fn try_clone(&self) -> io::Result<Socket> {
        Ok(Socket { fd: self.fd.try_clone()? })
    }
    pub(crate) fn name(&self, peer: bool) -> io::Result<(sn::SockaddrStorage, sn::socklen_t)> {
        let mut buf = sn::zeroed();
        let mut len: sn::socklen_t = 128;
        let r = unsafe {
            if peer {
                sn::getpeername(self.raw(), sn::storage_mut_ptr(&mut buf), &mut len as *mut sn::socklen_t)
            } else {
                sn::getsockname(self.raw(), sn::storage_mut_ptr(&mut buf), &mut len as *mut sn::socklen_t)
            }
        };
        cvt(r)?;
        Ok((buf, len))
    }
    pub(crate) fn accept(&self) -> io::Result<(Socket, sn::SockaddrStorage, sn::socklen_t)> {
        let mut buf = sn::zeroed();
        let mut len: sn::socklen_t = 128;
        loop {
            let r = unsafe { sn::accept(self.raw(), sn::storage_mut_ptr(&mut buf), &mut len as *mut sn::socklen_t) };
            if r >= 0 {
                let s = Socket::from_fd(r);
                unsafe {
                    sys::fcntl(r, sys::F_SETFD, sys::FD_CLOEXEC);
                }
                // SO_NOSIGPIPE is inherited from the listener (setting it again fails with
                // EINVAL once the peer has already closed)
                return Ok((s, buf, len));
            }
            let e = io::Error::last_os_error();
            if !e.is_interrupted() {
                return Err(e);
            }
        }
    }
    /// Blocking connect, retried on EINTR.
    pub(crate) fn connect(&self, buf: &sn::SockaddrStorage, len: sn::socklen_t) -> io::Result<()> {
        loop {
            let r = unsafe { sn::connect(self.raw(), sn::storage_ptr(buf), len) };
            if r == 0 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            if !e.is_interrupted() {
                return Err(e);
            }
        }
    }
    /// Non-blocking connect + poll for writability (real std's connect_timeout).
    fn connect_timeout(&self, buf: &sn::SockaddrStorage, len: sn::socklen_t, timeout: Duration) -> io::Result<()> {
        self.set_nonblocking(true)?;
        let r = unsafe { sn::connect(self.raw(), sn::storage_ptr(buf), len) };
        self.set_nonblocking(false)?;
        if r == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(sys::os::EINPROGRESS) {
            return Err(e);
        }
        if timeout.is_zero() {
            return Err(io::Error::ZERO_TIMEOUT);
        }
        let start = crate::time::Instant::now();
        loop {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return Err(io::Error::const_msg(io::ErrorKind::TimedOut, "connection timed out"));
            }
            let left = timeout - elapsed;
            let mut ms = left.as_millis();
            if ms == 0 {
                ms = 1;
            }
            let ms = if ms > c_int::MAX as u128 { c_int::MAX } else { ms as c_int };
            let mut pfd = sn::PollFd { fd: self.raw(), events: sn::POLLOUT, revents: 0 };
            let n = unsafe { sn::poll(&mut pfd as *mut sn::PollFd, 1, ms) };
            if n == -1 {
                let e = io::Error::last_os_error();
                if !e.is_interrupted() {
                    return Err(e);
                }
                continue;
            }
            if n == 0 {
                continue;
            }
            if pfd.revents & (0x8 | 0x10) != 0 {
                // POLLERR | POLLHUP
                match self.take_error()? {
                    Some(e) => return Err(e),
                    None => return Err(io::Error::const_msg(io::ErrorKind::Uncategorized, "no error set after POLLHUP")),
                }
            }
            return Ok(());
        }
    }
}

fn each_addr<A: ToSocketAddrs, T, F: FnMut(&SocketAddr) -> io::Result<T>>(addr: A, mut f: F) -> io::Result<T> {
    let addrs = match addr.to_socket_addrs() {
        Ok(a) => a,
        Err(e) => return Err(e),
    };
    let mut last_err = None;
    for a in addrs {
        match f(&a) {
            Ok(v) => return Ok(v),
            Err(e) => last_err = Some(e),
        }
    }
    match last_err {
        Some(e) => Err(e),
        None => Err(io::Error::NO_ADDRESSES),
    }
}

fn family_of(a: &SocketAddr) -> c_int {
    match a {
        SocketAddr::V4(_) => sn::AF_INET,
        SocketAddr::V6(_) => sn::AF_INET6,
    }
}

pub struct TcpStream {
    sock: Socket,
}

pub struct TcpListener {
    sock: Socket,
}

pub struct Incoming<'a> {
    listener: &'a TcpListener,
}

impl TcpStream {
    pub fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<TcpStream> {
        each_addr(addr, |a| {
            let s = Socket::new(family_of(a), sn::SOCK_STREAM)?;
            let mut buf = sn::zeroed();
            let len = a.encode(&mut buf);
            s.connect(&buf, len)?;
            Ok(TcpStream { sock: s })
        })
    }
    pub fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
        let s = Socket::new(family_of(addr), sn::SOCK_STREAM)?;
        let mut buf = sn::zeroed();
        let len = addr.encode(&mut buf);
        s.connect_timeout(&buf, len, timeout)?;
        Ok(TcpStream { sock: s })
    }
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        let (buf, _) = self.sock.name(true)?;
        SocketAddr::decode(&buf)
    }
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        let (buf, _) = self.sock.name(false)?;
        SocketAddr::decode(&buf)
    }
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.sock.shutdown(how)
    }
    pub fn try_clone(&self) -> io::Result<TcpStream> {
        Ok(TcpStream { sock: self.sock.try_clone()? })
    }
    pub fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.sock.set_timeout(dur, sn::SO_RCVTIMEO)
    }
    pub fn set_write_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.sock.set_timeout(dur, sn::SO_SNDTIMEO)
    }
    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        self.sock.timeout(sn::SO_RCVTIMEO)
    }
    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        self.sock.timeout(sn::SO_SNDTIMEO)
    }
    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.sock.peek(buf)
    }
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.sock.set_int(sn::IPPROTO_TCP, sn::TCP_NODELAY, nodelay as c_int)
    }
    pub fn nodelay(&self) -> io::Result<bool> {
        Ok(self.sock.get_int(sn::IPPROTO_TCP, sn::TCP_NODELAY)? != 0)
    }
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.sock.set_int(sn::IPPROTO_IP, sn::IP_TTL, ttl as c_int)
    }
    pub fn ttl(&self) -> io::Result<u32> {
        Ok(self.sock.get_int(sn::IPPROTO_IP, sn::IP_TTL)? as u32)
    }
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.sock.take_error()
    }
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.sock.set_nonblocking(nonblocking)
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.sock.read(buf)
    }
}
impl Read for &TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.sock.read(buf)
    }
}
impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sock.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Write for &TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sock.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut d = f.debug_struct("TcpStream");
        if let Ok(a) = self.local_addr() {
            d.field("addr", &a);
        }
        if let Ok(p) = self.peer_addr() {
            d.field("peer", &p);
        }
        d.field("fd", &self.sock.raw()).finish()
    }
}

impl TcpListener {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<TcpListener> {
        each_addr(addr, |a| {
            let s = Socket::new(family_of(a), sn::SOCK_STREAM)?;
            s.set_int(sn::SOL_SOCKET, sn::SO_REUSEADDR, 1)?;
            let mut buf = sn::zeroed();
            let len = a.encode(&mut buf);
            cvt(unsafe { sn::bind(s.raw(), sn::storage_ptr(&buf), len) })?;
            let backlog = if cfg!(target_os = "macos") { 128 } else { -1 };
            cvt(unsafe { sn::listen(s.raw(), backlog) })?;
            Ok(TcpListener { sock: s })
        })
    }
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        let (buf, _) = self.sock.name(false)?;
        SocketAddr::decode(&buf)
    }
    pub fn try_clone(&self) -> io::Result<TcpListener> {
        Ok(TcpListener { sock: self.sock.try_clone()? })
    }
    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let (s, buf, _) = self.sock.accept()?;
        Ok((TcpStream { sock: s }, SocketAddr::decode(&buf)?))
    }
    pub fn incoming(&self) -> Incoming<'_> {
        Incoming { listener: self }
    }
    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.sock.set_int(sn::IPPROTO_IP, sn::IP_TTL, ttl as c_int)
    }
    pub fn ttl(&self) -> io::Result<u32> {
        Ok(self.sock.get_int(sn::IPPROTO_IP, sn::IP_TTL)? as u32)
    }
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.sock.take_error()
    }
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.sock.set_nonblocking(nonblocking)
    }
}

impl<'a> Iterator for Incoming<'a> {
    type Item = io::Result<TcpStream>;
    fn next(&mut self) -> Option<io::Result<TcpStream>> {
        Some(match self.listener.accept() {
            Ok((s, _)) => Ok(s),
            Err(e) => Err(e),
        })
    }
}

impl fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut d = f.debug_struct("TcpListener");
        if let Ok(a) = self.local_addr() {
            d.field("addr", &a);
        }
        d.field("fd", &self.sock.raw()).finish()
    }
}

impl AsRawFd for TcpStream {
    fn as_raw_fd(&self) -> RawFd {
        self.sock.raw()
    }
}
impl AsRawFd for TcpListener {
    fn as_raw_fd(&self) -> RawFd {
        self.sock.raw()
    }
}
impl AsFd for TcpStream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.sock.owned().as_fd()
    }
}
impl AsFd for TcpListener {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.sock.owned().as_fd()
    }
}
impl FromRawFd for TcpStream {
    unsafe fn from_raw_fd(fd: RawFd) -> TcpStream {
        TcpStream { sock: Socket::from_fd(fd) }
    }
}
impl FromRawFd for TcpListener {
    unsafe fn from_raw_fd(fd: RawFd) -> TcpListener {
        TcpListener { sock: Socket::from_fd(fd) }
    }
}
impl IntoRawFd for TcpStream {
    fn into_raw_fd(self) -> RawFd {
        self.sock.into_owned().into_raw_fd()
    }
}
impl IntoRawFd for TcpListener {
    fn into_raw_fd(self) -> RawFd {
        self.sock.into_owned().into_raw_fd()
    }
}
impl From<TcpStream> for OwnedFd {
    fn from(s: TcpStream) -> OwnedFd {
        s.sock.into_owned()
    }
}
impl From<OwnedFd> for TcpStream {
    fn from(fd: OwnedFd) -> TcpStream {
        TcpStream { sock: Socket { fd } }
    }
}
