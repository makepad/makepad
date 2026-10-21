//! The socket under TcpStream / TcpListener / UdpSocket, web half: rustc's unsupported sockets
//! (`TcpStream(!)`, `UdpSocket(!)`): no socket can be made, so a `Socket` is uninhabited.
//! Same methods as socket_unix.rs / socket_windows.rs.

use core::ffi::c_int;
use core::time::Duration;

use super::Shutdown;
use crate::io;
use crate::sys::net as sn;

/// The Debug field name of the raw socket.
pub(crate) const RAW_FIELD: &str = "fd";

/// listen(2) backlog.
pub(crate) const BACKLOG: c_int = -1;

pub(crate) struct Socket(!);

impl Socket {
    pub(crate) fn new(_family: c_int, _ty: c_int) -> io::Result<Socket> {
        Err(io::Error::UNSUPPORTED_PLATFORM)
    }
    pub(crate) fn raw(&self) -> c_int {
        self.0
    }
    pub(crate) fn set_int(&self, _level: c_int, _name: c_int, _v: c_int) -> io::Result<()> {
        self.0
    }
    pub(crate) fn get_int(&self, _level: c_int, _name: c_int) -> io::Result<c_int> {
        self.0
    }
    pub(crate) fn read(&self, _buf: &mut [u8]) -> io::Result<usize> {
        self.0
    }
    pub(crate) fn peek(&self, _buf: &mut [u8]) -> io::Result<usize> {
        self.0
    }
    pub(crate) fn write(&self, _buf: &[u8]) -> io::Result<usize> {
        self.0
    }
    pub(crate) fn shutdown(&self, _how: Shutdown) -> io::Result<()> {
        self.0
    }
    pub(crate) fn set_nonblocking(&self, _nonblocking: bool) -> io::Result<()> {
        self.0
    }
    pub(crate) fn set_timeout(&self, _dur: Option<Duration>, _kind: c_int) -> io::Result<()> {
        self.0
    }
    pub(crate) fn timeout(&self, _kind: c_int) -> io::Result<Option<Duration>> {
        self.0
    }
    pub(crate) fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.0
    }
    pub(crate) fn try_clone(&self) -> io::Result<Socket> {
        self.0
    }
    pub(crate) fn name(&self, _peer: bool) -> io::Result<(sn::SockaddrStorage, sn::socklen_t)> {
        self.0
    }
    pub(crate) fn accept(&self) -> io::Result<(Socket, sn::SockaddrStorage, sn::socklen_t)> {
        self.0
    }
    pub(crate) fn connect(&self, _buf: &sn::SockaddrStorage, _len: sn::socklen_t) -> io::Result<()> {
        self.0
    }
    pub(crate) fn connect_timeout(&self, _buf: &sn::SockaddrStorage, _len: sn::socklen_t, _timeout: Duration) -> io::Result<()> {
        self.0
    }
    pub(crate) fn bind(&self, _buf: &sn::SockaddrStorage, _len: sn::socklen_t) -> io::Result<()> {
        self.0
    }
    pub(crate) fn listen(&self, _backlog: c_int) -> io::Result<()> {
        self.0
    }
    pub(crate) fn set_raw_opt(&self, _level: c_int, _name: c_int, _value: *const u8, _len: usize) -> io::Result<()> {
        self.0
    }
    pub(crate) fn recv_from(&self, _buf: &mut [u8], _peek: bool) -> io::Result<(usize, sn::SockaddrStorage)> {
        self.0
    }
    pub(crate) fn send_to(&self, _buf: &[u8], _addr: &sn::SockaddrStorage, _len: sn::socklen_t) -> io::Result<usize> {
        self.0
    }
}
