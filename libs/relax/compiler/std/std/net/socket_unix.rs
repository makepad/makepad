//! The socket under TcpStream / TcpListener / UdpSocket (and UnixStream), unix half: an
//! OwnedFd over BSD sockets. socket_windows.rs is the Winsock half with the same methods.

use core::ffi::{c_int, c_void};
use core::time::Duration;

use super::Shutdown;
use crate::io;
use crate::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use crate::sys;
use crate::sys::net as sn;

/// The Debug field name of the raw socket.
pub(crate) const RAW_FIELD: &str = "fd";

/// listen(2) backlog.
pub(crate) const BACKLOG: c_int = if cfg!(target_os = "macos") { 128 } else { -1 };

/// A socket fd with the helpers TcpStream/TcpListener/UnixStream share.
pub(crate) struct Socket {
    fd: OwnedFd,
}

pub(crate) fn cvt(r: c_int) -> io::Result<c_int> {
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
    pub(crate) fn from_owned(fd: OwnedFd) -> Socket {
        Socket { fd }
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
    pub(crate) fn connect_timeout(&self, buf: &sn::SockaddrStorage, len: sn::socklen_t, timeout: Duration) -> io::Result<()> {
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
                return Err(io::const_error!(io::ErrorKind::TimedOut, "connection timed out"));
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
                    None => return Err(io::const_error!(io::ErrorKind::Uncategorized, "no error set after POLLHUP")),
                }
            }
            return Ok(());
        }
    }
    pub(crate) fn bind(&self, buf: &sn::SockaddrStorage, len: sn::socklen_t) -> io::Result<()> {
        cvt(unsafe { sn::bind(self.raw(), sn::storage_ptr(buf), len) })?;
        Ok(())
    }
    pub(crate) fn listen(&self, backlog: c_int) -> io::Result<()> {
        cvt(unsafe { sn::listen(self.raw(), backlog) })?;
        Ok(())
    }
    /// setsockopt with an arbitrary option value (ip_mreq / ipv6_mreq).
    pub(crate) fn set_raw_opt(&self, level: c_int, name: c_int, value: *const u8, len: usize) -> io::Result<()> {
        cvt(unsafe { sn::setsockopt(self.raw(), level, name, value as *const c_void, len as sn::socklen_t) })?;
        Ok(())
    }
    /// recvfrom: the byte count and the sender's address.
    pub(crate) fn recv_from(&self, buf: &mut [u8], peek: bool) -> io::Result<(usize, sn::SockaddrStorage)> {
        let mut addr = sn::zeroed();
        let mut len: sn::socklen_t = 128;
        let flags = if peek { sn::MSG_PEEK } else { 0 };
        let r = unsafe {
            sn::recvfrom(self.raw(), buf.as_mut_ptr() as *mut c_void, buf.len(), flags,
                sn::storage_mut_ptr(&mut addr), &mut len as *mut sn::socklen_t)
        };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok((r as usize, addr))
        }
    }
    pub(crate) fn send_to(&self, buf: &[u8], addr: &sn::SockaddrStorage, len: sn::socklen_t) -> io::Result<usize> {
        let r = unsafe {
            sn::sendto(self.raw(), buf.as_ptr() as *const c_void, buf.len(), sn::MSG_NOSIGNAL,
                sn::storage_ptr(addr), len)
        };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(r as usize)
        }
    }
}
