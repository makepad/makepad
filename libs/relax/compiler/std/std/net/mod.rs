//! std::net: IP/socket addresses, TCP and UDP.

mod addr;
mod ip;
mod parser;
#[cfg(unix)]
#[path = "socket_unix.rs"]
mod socket;
#[cfg(windows)]
#[path = "socket_windows.rs"]
mod socket;
pub(crate) mod tcp;
mod udp;

pub use self::addr::{SocketAddr, SocketAddrV4, SocketAddrV6, ToSocketAddrs};
pub use self::ip::{IpAddr, Ipv4Addr, Ipv6Addr};
pub use self::parser::AddrParseError;
pub use self::tcp::{Incoming, TcpListener, TcpStream};
pub use self::udp::UdpSocket;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Shutdown {
    Read,
    Write,
    Both,
}
