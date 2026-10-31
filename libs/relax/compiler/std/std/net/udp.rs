//! UdpSocket over the OS socket (super::socket: BSD sockets or Winsock).

use core::ffi::c_int;
use core::fmt;
use core::time::Duration;

use super::socket::RAW_FIELD;
use super::tcp::{each_addr, family_of, Socket};
use super::{Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use crate::io;
use crate::sys::net as sn;

pub struct UdpSocket {
    sock: Socket,
}

impl UdpSocket {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<UdpSocket> {
        each_addr(addr, |a| {
            let s = Socket::new(family_of(a), sn::SOCK_DGRAM)?;
            let mut buf = sn::zeroed();
            let len = a.encode(&mut buf);
            s.bind(&buf, len)?;
            Ok(UdpSocket { sock: s })
        })
    }
    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let (n, addr) = self.sock.recv_from(buf, false)?;
        Ok((n, SocketAddr::decode(&addr)?))
    }
    pub fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let (n, addr) = self.sock.recv_from(buf, true)?;
        Ok((n, SocketAddr::decode(&addr)?))
    }
    pub fn send_to<A: ToSocketAddrs>(&self, buf: &[u8], addr: A) -> io::Result<usize> {
        let addr = match addr.to_socket_addrs()?.next() {
            Some(a) => a,
            None => return Err(io::const_error!(io::ErrorKind::InvalidInput, "no addresses to send data to")),
        };
        let mut storage = sn::zeroed();
        let len = addr.encode(&mut storage);
        self.sock.send_to(buf, &storage, len)
    }
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        let (buf, _) = self.sock.name(true)?;
        SocketAddr::decode(&buf)
    }
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        let (buf, _) = self.sock.name(false)?;
        SocketAddr::decode(&buf)
    }
    pub fn try_clone(&self) -> io::Result<UdpSocket> {
        Ok(UdpSocket { sock: self.sock.try_clone()? })
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
    pub fn set_broadcast(&self, broadcast: bool) -> io::Result<()> {
        self.sock.set_int(sn::SOL_SOCKET, sn::SO_BROADCAST, broadcast as c_int)
    }
    pub fn broadcast(&self) -> io::Result<bool> {
        Ok(self.sock.get_int(sn::SOL_SOCKET, sn::SO_BROADCAST)? != 0)
    }
    pub fn set_multicast_loop_v4(&self, multicast_loop_v4: bool) -> io::Result<()> {
        self.sock.set_int(sn::IPPROTO_IP, sn::IP_MULTICAST_LOOP, multicast_loop_v4 as c_int)
    }
    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        Ok(self.sock.get_int(sn::IPPROTO_IP, sn::IP_MULTICAST_LOOP)? != 0)
    }
    pub fn set_multicast_ttl_v4(&self, multicast_ttl_v4: u32) -> io::Result<()> {
        self.sock.set_int(sn::IPPROTO_IP, sn::IP_MULTICAST_TTL, multicast_ttl_v4 as c_int)
    }
    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        Ok(self.sock.get_int(sn::IPPROTO_IP, sn::IP_MULTICAST_TTL)? as u32)
    }
    pub fn set_multicast_loop_v6(&self, multicast_loop_v6: bool) -> io::Result<()> {
        self.sock.set_int(sn::IPPROTO_IPV6, sn::IPV6_MULTICAST_LOOP, multicast_loop_v6 as c_int)
    }
    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        Ok(self.sock.get_int(sn::IPPROTO_IPV6, sn::IPV6_MULTICAST_LOOP)? != 0)
    }
    fn membership_v4(&self, multiaddr: &Ipv4Addr, interface: &Ipv4Addr, name: c_int) -> io::Result<()> {
        // ip_mreq: two network-order in_addr values.
        let req = [u32::from_ne_bytes(multiaddr.octets()), u32::from_ne_bytes(interface.octets())];
        self.sock.set_raw_opt(sn::IPPROTO_IP, name, &req as *const [u32; 2] as *const u8, 8)
    }
    fn membership_v6(&self, multiaddr: &Ipv6Addr, interface: u32, name: c_int) -> io::Result<()> {
        // ipv6_mreq: 16 network-order address bytes followed by the native interface index.
        let ip = multiaddr.octets();
        let req = [
            u32::from_ne_bytes([ip[0], ip[1], ip[2], ip[3]]),
            u32::from_ne_bytes([ip[4], ip[5], ip[6], ip[7]]),
            u32::from_ne_bytes([ip[8], ip[9], ip[10], ip[11]]),
            u32::from_ne_bytes([ip[12], ip[13], ip[14], ip[15]]),
            interface,
        ];
        self.sock.set_raw_opt(sn::IPPROTO_IPV6, name, &req as *const [u32; 5] as *const u8, 20)
    }
    pub fn join_multicast_v4(&self, multiaddr: &Ipv4Addr, interface: &Ipv4Addr) -> io::Result<()> {
        self.membership_v4(multiaddr, interface, sn::IP_ADD_MEMBERSHIP)
    }
    pub fn join_multicast_v6(&self, multiaddr: &Ipv6Addr, interface: u32) -> io::Result<()> {
        self.membership_v6(multiaddr, interface, sn::IPV6_ADD_MEMBERSHIP)
    }
    pub fn leave_multicast_v4(&self, multiaddr: &Ipv4Addr, interface: &Ipv4Addr) -> io::Result<()> {
        self.membership_v4(multiaddr, interface, sn::IP_DROP_MEMBERSHIP)
    }
    pub fn leave_multicast_v6(&self, multiaddr: &Ipv6Addr, interface: u32) -> io::Result<()> {
        self.membership_v6(multiaddr, interface, sn::IPV6_DROP_MEMBERSHIP)
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
    pub fn connect<A: ToSocketAddrs>(&self, addr: A) -> io::Result<()> {
        each_addr(addr, |a| {
            let mut buf = sn::zeroed();
            let len = a.encode(&mut buf);
            self.sock.connect(&buf, len)
        })
    }
    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.sock.write(buf)
    }
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.sock.read(buf)
    }
    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.sock.peek(buf)
    }
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.sock.set_nonblocking(nonblocking)
    }
}

impl fmt::Debug for UdpSocket {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut d = f.debug_struct("UdpSocket");
        if let Ok(a) = self.local_addr() {
            d.field("addr", &a);
        }
        d.field(RAW_FIELD, &self.sock.raw()).finish()
    }
}

/// Unix: the fd traits of std::os::fd.
#[cfg(unix)]
mod os_impls {
    use super::{Socket, UdpSocket};
    use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};

    impl AsRawFd for UdpSocket {
        fn as_raw_fd(&self) -> RawFd {
            self.sock.raw()
        }
    }
    impl AsFd for UdpSocket {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.sock.owned().as_fd()
        }
    }
    impl FromRawFd for UdpSocket {
        unsafe fn from_raw_fd(fd: RawFd) -> UdpSocket {
            UdpSocket { sock: Socket::from_fd(fd) }
        }
    }
    impl IntoRawFd for UdpSocket {
        fn into_raw_fd(self) -> RawFd {
            self.sock.into_owned().into_raw_fd()
        }
    }
    impl From<UdpSocket> for OwnedFd {
        fn from(s: UdpSocket) -> OwnedFd {
            s.sock.into_owned()
        }
    }
    impl From<OwnedFd> for UdpSocket {
        fn from(fd: OwnedFd) -> UdpSocket {
            UdpSocket { sock: Socket::from_fd(fd.into_raw_fd()) }
        }
    }
}

/// Windows: the socket traits of std::os::windows::io.
#[cfg(windows)]
mod os_impls {
    use super::{Socket, UdpSocket};
    use crate::os::windows::io::{AsRawSocket, AsSocket, BorrowedSocket, FromRawSocket, IntoRawSocket, OwnedSocket, RawSocket};

    impl AsRawSocket for UdpSocket {
        fn as_raw_socket(&self) -> RawSocket {
            self.sock.raw()
        }
    }
    impl AsSocket for UdpSocket {
        fn as_socket(&self) -> BorrowedSocket<'_> {
            self.sock.owned().as_socket()
        }
    }
    impl FromRawSocket for UdpSocket {
        unsafe fn from_raw_socket(sock: RawSocket) -> UdpSocket {
            UdpSocket { sock: Socket::from_raw(sock) }
        }
    }
    impl IntoRawSocket for UdpSocket {
        fn into_raw_socket(self) -> RawSocket {
            self.sock.into_owned().into_raw_socket()
        }
    }
    impl From<UdpSocket> for OwnedSocket {
        fn from(s: UdpSocket) -> OwnedSocket {
            s.sock.into_owned()
        }
    }
    impl From<OwnedSocket> for UdpSocket {
        fn from(sock: OwnedSocket) -> UdpSocket {
            UdpSocket { sock: Socket::from_owned(sock) }
        }
    }
}
