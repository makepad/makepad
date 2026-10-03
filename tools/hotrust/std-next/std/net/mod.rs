//! std::net: IP/socket addresses, TCP.

mod addr;
mod ip;
mod parser;
pub(crate) mod tcp;

pub use self::addr::{SocketAddr, SocketAddrV4, SocketAddrV6, ToSocketAddrs};
pub use self::ip::{IpAddr, Ipv4Addr, Ipv6Addr};
pub use self::parser::AddrParseError;
pub use self::tcp::{Incoming, TcpListener, TcpStream};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Shutdown {
    Read,
    Write,
    Both,
}
