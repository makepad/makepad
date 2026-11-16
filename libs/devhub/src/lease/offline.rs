//! The test spool's sandbox: a fresh user and network namespace, only loopback up (11-15).
//! Entered before exec, inherited by the whole run; failure has no networked fallback.

#![cfg(target_os = "linux")]

use std::io;
use std::os::raw::{c_char, c_int, c_ulong, c_void};

extern "C" {
    fn getuid() -> u32;
    fn getgid() -> u32;
    fn unshare(flags: c_int) -> c_int;
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn close(fd: c_int) -> c_int;
    fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

/// All map text and paths are ready in the parent: the pre-exec child allocates nothing.
pub struct Prepared {
    writes: [(&'static [u8], Vec<u8>); 3],
}

impl Prepared {
    pub fn new() -> Self {
        let (uid, gid) = unsafe { (getuid(), getgid()) };
        Self { writes: [
            (b"/proc/self/setgroups\0", b"deny".to_vec()),
            (b"/proc/self/uid_map\0", format!("{uid} {uid} 1").into_bytes()),
            (b"/proc/self/gid_map\0", format!("{gid} {gid} 1").into_bytes()),
        ] }
    }
}

#[repr(C)]
struct IfReq {
    name: [u8; 16],
    flags: i16,
    pad: [u8; 22],
}

/// The forked child's sandbox setup: raw calls only, no allocation, locks or buffered I/O.
/// Every error aborts the spawn; there is no networked retry.
pub fn enter(p: &Prepared) -> io::Result<()> {
    unsafe {
        if unshare(0x10000000 | 0x40000000) < 0 { return Err(io::Error::last_os_error()); }
        for (path, bytes) in &p.writes {
            let fd = open(path.as_ptr().cast(), 1); // O_WRONLY
            if fd < 0 { return Err(io::Error::last_os_error()); }
            if write(fd, bytes.as_ptr().cast(), bytes.len()) != bytes.len() as isize {
                let e = io::Error::last_os_error();
                close(fd);
                return Err(e);
            }
            if close(fd) < 0 { return Err(io::Error::last_os_error()); }
        }
        let fd = socket(2, 2, 0); // AF_INET, SOCK_DGRAM
        if fd < 0 { return Err(io::Error::last_os_error()); }
        let mut req = IfReq { name: *b"lo\0\0\0\0\0\0\0\0\0\0\0\0\0\0", flags: 0, pad: [0; 22] };
        if ioctl(fd, 0x8913, &mut req as *mut IfReq) < 0 { // SIOCGIFFLAGS
            let e = io::Error::last_os_error();
            close(fd);
            return Err(e);
        }
        req.flags |= 1; // IFF_UP
        if ioctl(fd, 0x8914, &req as *const IfReq) < 0 { // SIOCSIFFLAGS
            let e = io::Error::last_os_error();
            close(fd);
            return Err(e);
        }
        if close(fd) < 0 { return Err(io::Error::last_os_error()); }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_works_without_an_outside_route() {
        use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
        use std::os::unix::process::ExitStatusExt;
        use std::time::Duration;
        extern "C" {
            fn fork() -> c_int;
            fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
            fn _exit(code: c_int) -> !;
        }
        let prepared = Prepared::new();
        let pid = unsafe { fork() };
        assert!(pid >= 0, "fork for the namespace proof: {}", io::Error::last_os_error());
        if pid == 0 {
            let code = (|| {
                if enter(&prepared).is_err() { return 1; }
                let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) { Ok(l) => l, Err(_) => return 2 };
                let addr = match listener.local_addr() { Ok(a) => a, Err(_) => return 3 };
                let _client = match TcpStream::connect(addr) { Ok(s) => s, Err(_) => return 4 };
                let _accepted = match listener.accept() { Ok(s) => s, Err(_) => return 5 };
                let outside = SocketAddr::from(([192, 0, 2, 1], 9));
                if TcpStream::connect_timeout(&outside, Duration::from_millis(100)).is_ok() { return 6; }
                0
            })();
            unsafe { _exit(code); }
        }
        let mut status = 0;
        loop {
            if unsafe { waitpid(pid, &mut status, 0) } == pid { break; }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted { panic!("wait for the namespace proof: {e}"); }
        }
        let status = std::process::ExitStatus::from_raw(status);
        let property = match status.code() {
            Some(0) => "loopback works and outside access fails",
            Some(1) => "enter the user and network namespace",
            Some(2) => "bind a loopback listener",
            Some(3) => "read the loopback listener's address",
            Some(4) => "connect to loopback",
            Some(5) => "accept the loopback connection",
            Some(6) => "outside access must fail: no route",
            _ => "namespace proof child exited normally",
        };
        assert!(status.success(), "{property}: {status}");
    }

    #[cfg(unix)]
    #[test]
    fn maps_keep_the_current_uid_and_gid() {
        let p = Prepared::new();
        let (uid, gid) = unsafe { (getuid(), getgid()) };
        assert_eq!(p.writes[0], (b"/proc/self/setgroups\0".as_slice(), b"deny".to_vec()), "deny supplementary groups before mapping");
        assert_eq!(p.writes[1], (b"/proc/self/uid_map\0".as_slice(), format!("{uid} {uid} 1").into_bytes()), "keep the uid inside the namespace");
        assert_eq!(p.writes[2], (b"/proc/self/gid_map\0".as_slice(), format!("{gid} {gid} 1").into_bytes()), "keep the gid inside the namespace");
    }
}
