//! core::net: IP and socket addresses and their parsing (rustc moved them here from std::net in
//! 1.77; std::net re-exports them).

mod addr;
mod ip;
mod parser;

pub use self::addr::{SocketAddr, SocketAddrV4, SocketAddrV6};
pub use self::ip::{IpAddr, Ipv4Addr, Ipv6Addr};
pub use self::parser::AddrParseError;
