//! UDP sockets: one several processes on one machine can bind at once, and one tied to a
//! network interface.

use std::io;
use std::net::{Ipv4Addr, UdpSocket};

/// Bind the discovery port so several clients on ONE host can listen at
/// once (VJ + AI Content + a game all discover the same server / fleet).
///
/// Unix: `SO_REUSEADDR` + `SO_REUSEPORT` before bind — on macOS and Linux,
/// BROADCAST datagrams (which beacons are) are delivered to every member of
/// the reuse group, so all apps see the server. Raw `extern "C"` because
/// std exposes no pre-bind socket options and this crate takes no external
/// dependencies.
///
/// Windows: deliberately NOT set. `SO_REUSEADDR` on Windows allows silent
/// socket hijacking by other processes (no `SO_REUSEPORT` equivalent with
/// safe semantics), so the port stays exclusive there; a second app's bind
/// fails with `AddrInUse`, which callers surface as an explicit "discovery
/// port already in use — configure explicit endpoints" condition rather
/// than a silent security hole.
#[cfg(unix)]
pub fn bind_reuse_udp(port: u16) -> std::io::Result<UdpSocket> {
    use std::os::unix::io::FromRawFd;

    const AF_INET: i32 = 2;
    const SOCK_DGRAM: i32 = 2;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const SOL_SOCKET: i32 = 0xffff;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    const SOL_SOCKET: i32 = 1;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const SO_REUSEADDR: i32 = 0x0004;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    const SO_REUSEADDR: i32 = 2;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const SO_REUSEPORT: i32 = 0x0200;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    const SO_REUSEPORT: i32 = 15;

    extern "C" {
        fn socket(domain: i32, ty: i32, protocol: i32) -> i32;
        fn setsockopt(
            fd: i32,
            level: i32,
            name: i32,
            value: *const core::ffi::c_void,
            len: u32,
        ) -> i32;
        fn bind(fd: i32, addr: *const u8, len: u32) -> i32;
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        fn close(fd: i32) -> i32;
    }

    unsafe {
        let fd = socket(AF_INET, SOCK_DGRAM, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fail = |fd: i32| -> std::io::Error {
            let e = std::io::Error::last_os_error();
            close(fd);
            e
        };
        // close-on-exec: a child the process spawns (a lease's command) must not inherit the
        // socket (std's own sockets are, this raw one was not)
        const F_SETFD: i32 = 2;
        const FD_CLOEXEC: i32 = 1;
        if fcntl(fd, F_SETFD, FD_CLOEXEC) != 0 {
            return Err(fail(fd));
        }
        let one: i32 = 1;
        let one_ptr = &one as *const i32 as *const core::ffi::c_void;
        let one_len = std::mem::size_of::<i32>() as u32;
        if setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, one_ptr, one_len) != 0 {
            return Err(fail(fd));
        }
        if setsockopt(fd, SOL_SOCKET, SO_REUSEPORT, one_ptr, one_len) != 0 {
            return Err(fail(fd));
        }
        // sockaddr_in for INADDR_ANY:port. BSD layouts carry a leading
        // sin_len byte; Linux uses a 16-bit sin_family.
        let mut addr = [0u8; 16];
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd"
        ))]
        {
            addr[0] = 16; // sin_len
            addr[1] = AF_INET as u8;
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd"
        )))]
        {
            addr[0..2].copy_from_slice(&(AF_INET as u16).to_ne_bytes());
        }
        addr[2..4].copy_from_slice(&port.to_be_bytes());
        // sin_addr stays 0.0.0.0.
        if bind(fd, addr.as_ptr(), 16) != 0 {
            return Err(fail(fd));
        }
        Ok(UdpSocket::from_raw_fd(fd))
    }
}

#[cfg(not(unix))]
pub fn bind_reuse_udp(port: u16) -> std::io::Result<UdpSocket> {
    UdpSocket::bind(("0.0.0.0", port))
}

/// How [`bind_udp_on_interface`] tied a socket to its interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterfaceBind {
    /// The system sends and receives only through the interface, whatever the routes prefer
    /// (`IP_BOUND_IF` on Apple systems, `SO_BINDTODEVICE` on Linux and Android).
    Device,
    /// Only the interface's IPv4 address is bound: datagrams carry it as their source, but the
    /// routes still choose the way out (Linux without the privilege `SO_BINDTODEVICE` asks for,
    /// and other systems).
    Address(Ipv4Addr),
}

/// A UDP socket on `port` (0: any free one) tied to the network interface `name` (`en1`, `eth0`),
/// so its datagrams leave through that interface even when another one comes first in the routes.
/// It says how it was tied; an interface that doesn't exist is a `NotFound` error.
pub fn bind_udp_on_interface(name: &str, port: u16) -> io::Result<(UdpSocket, InterfaceBind)> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        use std::os::unix::io::AsRawFd;
        extern "C" {
            fn if_nametoindex(name: *const std::ffi::c_char) -> std::ffi::c_uint;
            fn setsockopt(fd: i32, level: i32, name: i32, value: *const core::ffi::c_void, len: u32) -> i32;
        }
        const IPPROTO_IP: i32 = 0;
        const IP_BOUND_IF: i32 = 25;
        let c_name = std::ffi::CString::new(name).map_err(|_| no_interface(name))?;
        let index = unsafe { if_nametoindex(c_name.as_ptr()) };
        if index == 0 {
            return Err(no_interface(name));
        }
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))?;
        let value: u32 = index;
        let set = unsafe {
            setsockopt(socket.as_raw_fd(), IPPROTO_IP, IP_BOUND_IF, &value as *const u32 as *const core::ffi::c_void, 4)
        };
        if set != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((socket, InterfaceBind::Device))
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::io::AsRawFd;
        extern "C" {
            fn setsockopt(fd: i32, level: i32, name: i32, value: *const core::ffi::c_void, len: u32) -> i32;
        }
        const SOL_SOCKET: i32 = 1;
        const SO_BINDTODEVICE: i32 = 25;
        const EPERM: i32 = 1;
        const ENODEV: i32 = 19;
        // IFNAMSIZ is 16 with its NUL.
        if name.is_empty() || name.len() >= 16 || name.contains('\0') {
            return Err(no_interface(name));
        }
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))?;
        let set = unsafe {
            setsockopt(socket.as_raw_fd(), SOL_SOCKET, SO_BINDTODEVICE, name.as_ptr() as *const core::ffi::c_void, name.len() as u32)
        };
        if set == 0 {
            return Ok((socket, InterfaceBind::Device));
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(ENODEV) => Err(no_interface(name)),
            // Older kernels keep the option for privileged processes: bind the address instead.
            Some(EPERM) => {
                drop(socket);
                bind_interface_address(name, port)
            }
            _ => Err(e),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "android")))]
    {
        bind_interface_address(name, port)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
fn bind_interface_address(name: &str, port: u16) -> io::Result<(UdpSocket, InterfaceBind)> {
    let addr = crate::netif::interfaces().into_iter().find(|i| i.name == name).map(|i| i.addr).ok_or_else(|| no_interface(name))?;
    Ok((UdpSocket::bind((addr, port))?, InterfaceBind::Address(addr)))
}

fn no_interface(name: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("no network interface named {name:?}"))
}

/// Waits, without a timeout, until `a` or `b` has a datagram to read: (a readable, b readable).
/// What makes a blocking listener stoppable: `b` is a private loopback socket its stopper sends
/// one datagram to.
#[cfg(unix)]
pub fn wait_readable(a: &UdpSocket, b: &UdpSocket) -> std::io::Result<(bool, bool)> {
    use std::os::unix::io::AsRawFd;
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    type NFds = std::ffi::c_uint;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    type NFds = std::ffi::c_ulong;
    extern "C" {
        fn poll(fds: *mut PollFd, n: NFds, timeout: i32) -> i32;
    }
    const POLLIN: i16 = 1;
    let mut fds = [
        PollFd { fd: a.as_raw_fd(), events: POLLIN, revents: 0 },
        PollFd { fd: b.as_raw_fd(), events: POLLIN, revents: 0 },
    ];
    loop {
        let r = unsafe { poll(fds.as_mut_ptr(), 2, -1) };
        if r >= 0 {
            return Ok((fds[0].revents != 0, fds[1].revents != 0));
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::io::AsRawFd;

    extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }

    #[test]
    fn a_socket_tied_to_an_interface_says_how_and_a_missing_interface_is_not_found() {
        let missing = bind_udp_on_interface("no-such-if0", 0).map(|_| ()).unwrap_err();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound, "a name no interface has is NotFound: {missing}");
        // Any up interface of this machine binds, by device where the system allows it.
        if let Some(interface) = crate::netif::interfaces().first() {
            let (socket, how) = bind_udp_on_interface(&interface.name, 0).expect("an up interface binds");
            assert!(socket.local_addr().unwrap().port() != 0, "it has a port");
            assert!(matches!(how, InterfaceBind::Device | InterfaceBind::Address(_)), "it says how it was tied: {how:?}");
            if let InterfaceBind::Address(addr) = how {
                assert_eq!(addr, interface.addr, "a bind by address uses that interface's own address");
            }
        }
    }

    #[test]
    fn the_reuse_socket_is_close_on_exec() {
        let s = bind_reuse_udp(0).unwrap();
        const F_GETFD: i32 = 1;
        let flags = unsafe { fcntl(s.as_raw_fd(), F_GETFD) };
        assert!(flags >= 0 && flags & 1 == 1, "FD_CLOEXEC not set (flags {flags})");
    }
}
