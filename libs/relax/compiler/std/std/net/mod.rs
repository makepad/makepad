//! std::net: IP/socket addresses, TCP and UDP.

mod addr;
#[cfg(unix)]
#[path = "socket_unix.rs"]
mod socket;
#[cfg(windows)]
#[path = "socket_windows.rs"]
mod socket;
#[cfg(target_family = "wasm")]
#[path = "socket_web.rs"]
mod socket;
pub(crate) mod tcp;
mod udp;

pub use self::addr::ToSocketAddrs;
pub use core::net::{AddrParseError, IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
pub use self::tcp::{Incoming, TcpListener, TcpStream};
pub use self::udp::UdpSocket;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Shutdown {
    Read,
    Write,
    Both,
}
