//! A UDP socket several processes on one machine can bind at once.

use std::net::UdpSocket;

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
