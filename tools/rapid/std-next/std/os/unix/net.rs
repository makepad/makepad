//! UnixStream / UnixListener (AF_UNIX stream sockets).

use core::ffi::c_int;
use core::fmt;
use core::time::Duration;

use crate::ffi::OsStr;
use crate::io::{self, Read, Write};
use crate::net::tcp::Socket;
use crate::net::Shutdown;
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::path::Path;
use crate::sys::net as sn;

#[derive(Clone)]
pub struct SocketAddr {
    path: alloc::vec::Vec<u8>,
}

impl SocketAddr {
    pub fn as_pathname(&self) -> Option<&Path> {
        if self.path.is_empty() {
            None
        } else {
            Some(Path::new(OsStr::from_raw_bytes(&self.path)))
        }
    }
    pub fn is_unnamed(&self) -> bool {
        self.path.is_empty()
    }
    pub fn from_pathname<P: AsRef<Path>>(path: P) -> io::Result<SocketAddr> {
        let b = path.as_ref().as_os_str().as_encoded_bytes();
        let mut buf = sn::zeroed();
        match sn::encode_unix(&mut buf, b) {
            Some(_) => Ok(SocketAddr { path: b.to_vec() }),
            None => Err(io::Error::const_msg(io::ErrorKind::InvalidInput, "path must be shorter than SUN_LEN")),
        }
    }
}

impl fmt::Debug for SocketAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.as_pathname() {
            Some(p) => write!(f, "{:?} (pathname)", p),
            None => f.write_str("(unnamed)"),
        }
    }
}

fn encode_path(path: &Path) -> io::Result<(sn::SockaddrStorage, sn::socklen_t)> {
    let b = path.as_os_str().as_encoded_bytes();
    for c in b {
        if *c == 0 {
            return Err(io::Error::const_msg(io::ErrorKind::InvalidInput, "paths must not contain interior null bytes"));
        }
    }
    let mut buf = sn::zeroed();
    match sn::encode_unix(&mut buf, b) {
        Some(len) => Ok((buf, len)),
        None => Err(io::Error::const_msg(io::ErrorKind::InvalidInput, "path must be shorter than SUN_LEN")),
    }
}

fn cvt(r: c_int) -> io::Result<c_int> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

pub struct UnixStream {
    sock: Socket,
}

pub struct UnixListener {
    sock: Socket,
}

pub struct Incoming<'a> {
    listener: &'a UnixListener,
}

impl UnixStream {
    pub fn connect<P: AsRef<Path>>(path: P) -> io::Result<UnixStream> {
        let (buf, len) = encode_path(path.as_ref())?;
        let s = Socket::new(sn::AF_UNIX, sn::SOCK_STREAM)?;
        s.connect(&buf, len)?;
        Ok(UnixStream { sock: s })
    }
    pub fn pair() -> io::Result<(UnixStream, UnixStream)> {
        let mut fds = [0 as c_int; 2];
        cvt(unsafe { sn::socketpair(sn::AF_UNIX, sn::SOCK_STREAM, 0, fds.as_mut_ptr()) })?;
        let a = Socket::from_fd(fds[0]);
        let b = Socket::from_fd(fds[1]);
        unsafe {
            crate::sys::fcntl(fds[0], crate::sys::F_SETFD, crate::sys::FD_CLOEXEC);
            crate::sys::fcntl(fds[1], crate::sys::F_SETFD, crate::sys::FD_CLOEXEC);
        }
        if cfg!(target_os = "macos") {
            a.set_int(sn::SOL_SOCKET, sn::SO_NOSIGPIPE, 1)?;
            b.set_int(sn::SOL_SOCKET, sn::SO_NOSIGPIPE, 1)?;
        }
        Ok((UnixStream { sock: a }, UnixStream { sock: b }))
    }
    pub fn try_clone(&self) -> io::Result<UnixStream> {
        Ok(UnixStream { sock: self.sock.try_clone()? })
    }
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        let (buf, len) = self.sock.name(false)?;
        Ok(SocketAddr { path: sn::decode_unix(&buf, len) })
    }
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        let (buf, len) = self.sock.name(true)?;
        Ok(SocketAddr { path: sn::decode_unix(&buf, len) })
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
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.sock.set_nonblocking(nonblocking)
    }
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.sock.take_error()
    }
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.sock.shutdown(how)
    }
    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.sock.peek(buf)
    }
}

impl Read for UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.sock.read(buf)
    }
}
impl Read for &UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.sock.read(buf)
    }
}
impl Write for UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sock.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Write for &UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sock.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl fmt::Debug for UnixStream {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut d = f.debug_struct("UnixStream");
        d.field("fd", &self.sock.raw());
        if let Ok(a) = self.local_addr() {
            d.field("local", &a);
        }
        if let Ok(p) = self.peer_addr() {
            d.field("peer", &p);
        }
        d.finish()
    }
}

impl UnixListener {
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<UnixListener> {
        let (buf, len) = encode_path(path.as_ref())?;
        let s = Socket::new(sn::AF_UNIX, sn::SOCK_STREAM)?;
        cvt(unsafe { sn::bind(s.raw(), sn::storage_ptr(&buf), len) })?;
        let backlog = if cfg!(target_os = "macos") { 128 } else { -1 };
        cvt(unsafe { sn::listen(s.raw(), backlog) })?;
        Ok(UnixListener { sock: s })
    }
    pub fn accept(&self) -> io::Result<(UnixStream, SocketAddr)> {
        let (s, buf, len) = self.sock.accept()?;
        Ok((UnixStream { sock: s }, SocketAddr { path: sn::decode_unix(&buf, len) }))
    }
    pub fn try_clone(&self) -> io::Result<UnixListener> {
        Ok(UnixListener { sock: self.sock.try_clone()? })
    }
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        let (buf, len) = self.sock.name(false)?;
        Ok(SocketAddr { path: sn::decode_unix(&buf, len) })
    }
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.sock.set_nonblocking(nonblocking)
    }
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.sock.take_error()
    }
    pub fn incoming(&self) -> Incoming<'_> {
        Incoming { listener: self }
    }
}

impl<'a> Iterator for Incoming<'a> {
    type Item = io::Result<UnixStream>;
    fn next(&mut self) -> Option<io::Result<UnixStream>> {
        Some(match self.listener.accept() {
            Ok((s, _)) => Ok(s),
            Err(e) => Err(e),
        })
    }
}

impl fmt::Debug for UnixListener {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut d = f.debug_struct("UnixListener");
        d.field("fd", &self.sock.raw());
        if let Ok(a) = self.local_addr() {
            d.field("local", &a);
        }
        d.finish()
    }
}

impl AsRawFd for UnixStream {
    fn as_raw_fd(&self) -> RawFd {
        self.sock.raw()
    }
}
impl AsRawFd for UnixListener {
    fn as_raw_fd(&self) -> RawFd {
        self.sock.raw()
    }
}
impl AsFd for UnixStream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.sock.owned().as_fd()
    }
}
impl AsFd for UnixListener {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.sock.owned().as_fd()
    }
}
impl FromRawFd for UnixStream {
    unsafe fn from_raw_fd(fd: RawFd) -> UnixStream {
        UnixStream { sock: Socket::from_fd(fd) }
    }
}
impl IntoRawFd for UnixStream {
    fn into_raw_fd(self) -> RawFd {
        self.sock.into_owned().into_raw_fd()
    }
}
impl From<UnixStream> for OwnedFd {
    fn from(s: UnixStream) -> OwnedFd {
        s.sock.into_owned()
    }
}
impl From<OwnedFd> for UnixStream {
    fn from(fd: OwnedFd) -> UnixStream {
        UnixStream { sock: Socket::from_fd(fd.into_raw_fd()) }
    }
}
