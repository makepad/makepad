//! Sockets, web half (rustc's sys/net/connection/unsupported.rs: no socket can be made). Only the
//! names net/{addr,tcp,udp}.rs and sys/net.rs's sockaddr encoders compile against; no value is
//! ever used to talk to anything.

use core::ffi::c_int;

pub type socklen_t = u32;

pub const AF_INET6: c_int = 10;
pub const SOL_SOCKET: c_int = 1;
pub const SO_REUSEADDR: c_int = 2;
pub const SO_BROADCAST: c_int = 6;
pub const SO_SNDTIMEO: c_int = 21;
pub const SO_RCVTIMEO: c_int = 20;
pub const IPPROTO_IP: c_int = 0;
pub const IP_TTL: c_int = 2;
pub const IP_MULTICAST_LOOP: c_int = 34;
pub const IP_MULTICAST_TTL: c_int = 33;
pub const IP_ADD_MEMBERSHIP: c_int = 35;
pub const IP_DROP_MEMBERSHIP: c_int = 36;
pub const IPV6_MULTICAST_LOOP: c_int = 19;
pub const IPV6_ADD_MEMBERSHIP: c_int = 20;
pub const IPV6_DROP_MEMBERSHIP: c_int = 21;
