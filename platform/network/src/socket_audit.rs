//! `MAKEPAD_NET_AUDIT=1`: log every socket this process holds, so a run can show it never touched
//! the local network (no debugger, no OS tracing: the process lists its own descriptors). A
//! sampler thread lists the process's socket descriptors every 20 ms and prints each new one once:
//!
//! `[net-audit] socket fd=12 udp local=0.0.0.0:41830 peer=- LAN`
//!
//! `LAN` marks what can ask macOS's Local Network question: a peer on the local network, or a UDP
//! socket bound off loopback (it can receive broadcasts or send to the LAN). Sockets that live less
//! than one sample are missed by the sampler; every LAN decision site also logs itself
//! (`[net-audit] lan allow|refuse <site> <target>`, see [`crate::lan`]).

use std::sync::atomic::{AtomicBool, Ordering};

pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_NET_AUDIT").is_ok_and(|v| v.trim() == "1"))
}

/// Starts the sampler once when `MAKEPAD_NET_AUDIT=1` (cheap to call from anywhere).
pub fn start_if_enabled() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if !enabled() || STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        eprintln!("[net-audit] on: pid {} (lan {})", std::process::id(), if crate::lan::allowed() { "on" } else { "off" });
        let _ = std::thread::Builder::new().name("net-audit".into()).spawn(sampler);
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn sampler() {
    use std::collections::HashSet;
    let mut seen: HashSet<(i32, String)> = HashSet::new();
    loop {
        let mut live = HashSet::new();
        for fd in sys::socket_fds() {
            let Some(line) = sys::describe(fd) else { continue };
            let key = (fd, line.clone());
            if !seen.contains(&key) {
                eprintln!("[net-audit] socket fd={fd} {line}");
            }
            live.insert(key);
        }
        seen = live;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod sys {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    #[cfg(target_os = "macos")]
    const AF_INET6: u8 = 30;
    #[cfg(target_os = "linux")]
    const AF_INET6: u16 = 10;
    const AF_INET: u8 = 2;
    #[cfg(target_os = "macos")]
    const SOL_SOCKET: i32 = 0xffff;
    #[cfg(target_os = "linux")]
    const SOL_SOCKET: i32 = 1;
    #[cfg(target_os = "macos")]
    const SO_TYPE: i32 = 0x1008;
    #[cfg(target_os = "linux")]
    const SO_TYPE: i32 = 3;

    extern "C" {
        fn getsockname(fd: i32, addr: *mut u8, len: *mut u32) -> i32;
        fn getpeername(fd: i32, addr: *mut u8, len: *mut u32) -> i32;
        fn getsockopt(fd: i32, level: i32, name: i32, value: *mut i32, len: *mut u32) -> i32;
        #[cfg(target_os = "macos")]
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut u8, size: i32) -> i32;
    }

    /// This process's socket descriptors.
    #[cfg(target_os = "macos")]
    pub fn socket_fds() -> Vec<i32> {
        const PROC_PIDLISTFDS: i32 = 1;
        const PROX_FDTYPE_SOCKET: u32 = 2;
        let pid = std::process::id() as i32;
        let mut buf = vec![0u8; 8 * 4096];
        let n = unsafe { proc_pidinfo(pid, PROC_PIDLISTFDS, 0, buf.as_mut_ptr(), buf.len() as i32) };
        if n <= 0 {
            return Vec::new();
        }
        buf[..n as usize]
            .chunks_exact(8)
            .filter(|e| u32::from_ne_bytes([e[4], e[5], e[6], e[7]]) == PROX_FDTYPE_SOCKET)
            .map(|e| i32::from_ne_bytes([e[0], e[1], e[2], e[3]]))
            .collect()
    }

    #[cfg(target_os = "linux")]
    pub fn socket_fds() -> Vec<i32> {
        let Ok(dir) = std::fs::read_dir("/proc/self/fd") else { return Vec::new() };
        dir.flatten()
            .filter(|e| std::fs::read_link(e.path()).is_ok_and(|t| t.to_string_lossy().starts_with("socket:")))
            .filter_map(|e| e.file_name().to_str()?.parse().ok())
            .collect()
    }

    fn parse(buf: &[u8], len: usize) -> Option<SocketAddr> {
        #[cfg(target_os = "macos")]
        let family = buf[1];
        #[cfg(target_os = "linux")]
        let family = u16::from_ne_bytes([buf[0], buf[1]]);
        let port = u16::from_be_bytes([buf[2], buf[3]]);
        if family as u16 == AF_INET as u16 && len >= 8 {
            let ip = Ipv4Addr::new(buf[4], buf[5], buf[6], buf[7]);
            return Some(SocketAddr::new(IpAddr::V4(ip), port));
        }
        if family as u16 == AF_INET6 as u16 && len >= 24 {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&buf[8..24]);
            return Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port));
        }
        None
    }

    fn name(fd: i32, peer: bool) -> Option<SocketAddr> {
        let mut buf = [0u8; 128];
        let mut len = buf.len() as u32;
        let rc = unsafe {
            if peer { getpeername(fd, buf.as_mut_ptr(), &mut len) } else { getsockname(fd, buf.as_mut_ptr(), &mut len) }
        };
        (rc == 0).then(|| parse(&buf, len as usize)).flatten()
    }

    /// One line for an IP socket (`None` for unix-domain and other sockets).
    pub fn describe(fd: i32) -> Option<String> {
        let local = name(fd, false)?;
        let peer = name(fd, true);
        let mut ty = 0i32;
        let mut len = 4u32;
        unsafe { getsockopt(fd, SOL_SOCKET, SO_TYPE, &mut ty, &mut len) };
        let udp = ty == 2;
        let lan_peer = peer.is_some_and(|p| crate::lan::is_local_network(p.ip()));
        let open_udp = udp && peer.is_none() && !local.ip().is_loopback();
        Some(format!(
            "{} local={local} peer={}{}",
            if udp { "udp" } else if ty == 1 { "tcp" } else { "ip" },
            peer.map_or("-".to_string(), |p| p.to_string()),
            if lan_peer || open_udp { " LAN" } else { "" }
        ))
    }
}
