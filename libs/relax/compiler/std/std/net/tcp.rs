//! TcpStream / TcpListener over the OS socket (super::socket: BSD sockets or Winsock).

use core::ffi::c_int;
use core::fmt;
use core::time::Duration;

use super::{Shutdown, SocketAddr, ToSocketAddrs};
use crate::io::{self, Read, Write};
use crate::sys::net as sn;

pub(crate) use super::socket::Socket;
use super::socket::{BACKLOG, RAW_FIELD};

pub(crate) fn each_addr<A: ToSocketAddrs, T, F: FnMut(&SocketAddr) -> io::Result<T>>(addr: A, mut f: F) -> io::Result<T> {
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

pub(crate) fn family_of(a: &SocketAddr) -> c_int {
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
        d.field(RAW_FIELD, &self.sock.raw()).finish()
    }
}

impl TcpListener {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<TcpListener> {
        each_addr(addr, |a| {
            let s = Socket::new(family_of(a), sn::SOCK_STREAM)?;
            // SO_REUSEADDR rebinds quickly on BSD sockets; on Windows it would let another
            // socket take over an address in use, so real std leaves it off there.
            if !cfg!(windows) {
                s.set_int(sn::SOL_SOCKET, sn::SO_REUSEADDR, 1)?;
            }
            let mut buf = sn::zeroed();
            let len = a.encode(&mut buf);
            s.bind(&buf, len)?;
            s.listen(BACKLOG)?;
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
        d.field(RAW_FIELD, &self.sock.raw()).finish()
    }
}

/// Unix: the fd traits of std::os::fd.
#[cfg(unix)]
mod os_impls {
    use super::{Socket, TcpListener, TcpStream};
    use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};

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
            TcpStream { sock: Socket::from_owned(fd) }
        }
    }
}

/// Windows: the socket traits of std::os::windows::io.
#[cfg(windows)]
mod os_impls {
    use super::{Socket, TcpListener, TcpStream};
    use crate::os::windows::io::{AsRawSocket, AsSocket, BorrowedSocket, FromRawSocket, IntoRawSocket, OwnedSocket, RawSocket};

    impl AsRawSocket for TcpStream {
        fn as_raw_socket(&self) -> RawSocket {
            self.sock.raw()
        }
    }
    impl AsRawSocket for TcpListener {
        fn as_raw_socket(&self) -> RawSocket {
            self.sock.raw()
        }
    }
    impl AsSocket for TcpStream {
        fn as_socket(&self) -> BorrowedSocket<'_> {
            self.sock.owned().as_socket()
        }
    }
    impl AsSocket for TcpListener {
        fn as_socket(&self) -> BorrowedSocket<'_> {
            self.sock.owned().as_socket()
        }
    }
    impl FromRawSocket for TcpStream {
        unsafe fn from_raw_socket(sock: RawSocket) -> TcpStream {
            TcpStream { sock: Socket::from_raw(sock) }
        }
    }
    impl FromRawSocket for TcpListener {
        unsafe fn from_raw_socket(sock: RawSocket) -> TcpListener {
            TcpListener { sock: Socket::from_raw(sock) }
        }
    }
    impl IntoRawSocket for TcpStream {
        fn into_raw_socket(self) -> RawSocket {
            self.sock.into_owned().into_raw_socket()
        }
    }
    impl IntoRawSocket for TcpListener {
        fn into_raw_socket(self) -> RawSocket {
            self.sock.into_owned().into_raw_socket()
        }
    }
    impl From<TcpStream> for OwnedSocket {
        fn from(s: TcpStream) -> OwnedSocket {
            s.sock.into_owned()
        }
    }
    impl From<OwnedSocket> for TcpStream {
        fn from(sock: OwnedSocket) -> TcpStream {
            TcpStream { sock: Socket::from_owned(sock) }
        }
    }
    impl From<TcpListener> for OwnedSocket {
        fn from(s: TcpListener) -> OwnedSocket {
            s.sock.into_owned()
        }
    }
    impl From<OwnedSocket> for TcpListener {
        fn from(sock: OwnedSocket) -> TcpListener {
            TcpListener { sock: Socket::from_owned(sock) }
        }
    }
}
