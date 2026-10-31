//! The network carrier: TLS on each OS's own stack with the tunnel's pre-shared-key proof, both
//! from makepad-network (the CI bridge uses the same). It is only opened by an explicit call:
//! [`listen`] for an endpoint that accepts paired peers, [`dial`] for a client. Nothing in this
//! crate opens a TCP port by default.
//!
//! A TLS stream cannot be split between a reader and a writer thread, so ONE I/O thread owns it.
//! It writes the pending outgoing bytes with a write timeout; when the write would block (the
//! peer is not draining) it reads with a short timeout, which drains the peer, and then retries
//! the same remaining buffer (every stack resumes an interrupted write that way, see the Write
//! impls in platform/network). Two peers that both fill their send buffers therefore cannot
//! stall each other. The frame reader and the connection's writer talk to this thread through a
//! small in-memory pipe.
//!
//! Between writes the thread waits for input on the socket itself (unix `poll`, next to a wake
//! socket the writer pokes when it queues bytes), so a frame goes out the moment it is queued: a
//! timed read in progress used to hold every outgoing frame for up to its timeout (20 ms per
//! side, ~40 ms per request on a LAN where a ping is under 1 ms). Without a socket handle (Windows,
//! Android) the thread falls back to the timed read.

use super::Carrier;
use crate::endpoint::Endpoint;
use crate::frame::FrameReader;
use makepad_network::tls::{TlsIdentity, TlsServer, TlsStream};
use makepad_network::tunnel::{connect_with_credentials, server_authenticate, ClientCredentials, Psk, TunnelConn};
use std::io::{self, Read, Write};
use std::net::{TcpListener, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const POLL: Duration = Duration::from_millis(20);
/// The longest wait for input on the socket (a queued frame or input ends it at once).
#[cfg(unix)]
const WAIT: Duration = Duration::from_millis(500);
const HANDSHAKE: Duration = Duration::from_secs(10);
/// outgoing bytes the connection's writer may have queued for the I/O thread
const OUT_CAP: usize = 256 << 10;

fn would_block(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted)
}

#[derive(Default)]
struct PipeState {
    inb: Vec<u8>,
    inpos: usize,
    out: Vec<u8>,
    closed: bool,
}

#[derive(Default)]
struct Pipe {
    st: Mutex<PipeState>,
    cv_in: Condvar,
    cv_out: Condvar,
    /// poked when bytes are queued (or the pipe closes): wakes the I/O thread's wait for input
    wake: Option<wait::Wake>,
}

impl Pipe {
    fn close(&self) {
        self.st.lock().unwrap().closed = true;
        self.cv_in.notify_all();
        self.cv_out.notify_all();
        if let Some(w) = &self.wake {
            w.poke();
        }
    }
}

/// A stream over a TCP socket the I/O thread can wait on for input.
pub trait Socket {
    /// A handle on the stream's TCP socket; None: the I/O thread waits in timed reads.
    fn tcp(&self) -> Option<std::net::TcpStream>;
}

impl Socket for TlsStream {
    fn tcp(&self) -> Option<std::net::TcpStream> {
        self.try_clone_tcp().ok()
    }
}

impl Socket for TunnelConn {
    fn tcp(&self) -> Option<std::net::TcpStream> {
        self.try_clone_tcp().ok()
    }
}

impl Socket for makepad_network::SocketStream {
    fn tcp(&self) -> Option<std::net::TcpStream> {
        self.try_clone_tcp().ok()
    }
}

/// The I/O thread's wait for input: `poll` on the socket and the wake socket (unix).
#[cfg(unix)]
mod wait {
    use std::io::{Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    const POLLIN: i16 = 0x1;
    const POLLOUT: i16 = 0x4;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    type Nfds = std::ffi::c_ulong;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    type Nfds = std::ffi::c_uint;
    extern "C" {
        fn poll(fds: *mut PollFd, n: Nfds, timeout_ms: i32) -> i32;
    }

    /// The writer's half pokes, the I/O thread's half is waited on with the socket.
    pub struct Wake {
        poke: UnixStream,
        pub(super) wait: UnixStream,
    }

    impl Wake {
        pub fn new() -> Option<Wake> {
            let (poke, wait) = UnixStream::pair().ok()?;
            poke.set_nonblocking(true).ok()?;
            wait.set_nonblocking(true).ok()?;
            Some(Wake { poke, wait })
        }
        /// A byte is enough: a full wake socket already wakes the thread.
        pub fn poke(&self) {
            let _ = (&self.poke).write(&[1]);
        }
    }

    /// What ended a wait.
    pub enum Ready {
        /// the socket has input (or hung up, or failed: the read says which)
        Input,
        /// bytes were queued (or the socket can take more when `writable` was asked)
        Write,
        Timeout,
    }

    /// Waits up to `t` for input on `sock`, a poke, or (with `writable`) room to write.
    pub fn wait(sock: &std::net::TcpStream, wake: &Wake, writable: bool, t: Duration) -> Ready {
        let mut fds = [PollFd { fd: sock.as_raw_fd(), events: POLLIN | if writable { POLLOUT } else { 0 }, revents: 0 }, PollFd { fd: wake.wait.as_raw_fd(), events: POLLIN, revents: 0 }];
        let n = unsafe { poll(fds.as_mut_ptr(), 2, t.as_millis().min(i32::MAX as u128) as i32) };
        if n < 0 {
            // EINTR and friends: the read decides
            return Ready::Input;
        }
        if n == 0 {
            return Ready::Timeout;
        }
        if fds[1].revents != 0 {
            let mut buf = [0u8; 64];
            while matches!((&wake.wait).read(&mut buf), Ok(n) if n > 0) {}
        }
        if fds[0].revents & !POLLOUT != 0 {
            return Ready::Input;
        }
        Ready::Write
    }
}

#[cfg(not(unix))]
mod wait {
    pub struct Wake;
    impl Wake {
        pub fn poke(&self) {}
    }
}

struct PipeRead(Arc<Pipe>);
struct PipeWrite(Arc<Pipe>);

impl Read for PipeRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut st = self.0.st.lock().unwrap();
        loop {
            if st.inpos < st.inb.len() {
                let n = buf.len().min(st.inb.len() - st.inpos);
                buf[..n].copy_from_slice(&st.inb[st.inpos..st.inpos + n]);
                st.inpos += n;
                if st.inpos == st.inb.len() {
                    st.inb.clear();
                    st.inpos = 0;
                }
                return Ok(n);
            }
            if st.closed {
                return Ok(0);
            }
            st = self.0.cv_in.wait(st).unwrap();
        }
    }
}

impl Write for PipeWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut st = self.0.st.lock().unwrap();
        while st.out.len() >= OUT_CAP && !st.closed {
            st = self.0.cv_out.wait(st).unwrap();
        }
        if st.closed {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "connection closed"));
        }
        let was_empty = st.out.is_empty();
        st.out.extend_from_slice(buf);
        drop(st);
        if was_empty && !buf.is_empty() {
            if let Some(w) = &self.0.wake {
                w.poke();
            }
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The I/O thread: owns `s` until the pipe closes or the stream fails; `close` shuts it down.
/// `sock`: the stream's TCP socket, waited on for input (with the pipe's wake) between writes.
fn io_loop<S: Read + Write>(mut s: S, sock: Option<std::net::TcpStream>, pipe: Arc<Pipe>, close: impl FnOnce(&mut S)) {
    let mut cur: Vec<u8> = Vec::new();
    let mut pos = 0;
    let mut buf = vec![0u8; 64 << 10];
    // the last read filled the buffer: the TLS stack may hold more than the socket shows
    let mut full = false;
    loop {
        if pos == cur.len() {
            cur.clear();
            pos = 0;
            let mut st = pipe.st.lock().unwrap();
            if st.closed {
                break;
            }
            std::mem::swap(&mut cur, &mut st.out);
            if !cur.is_empty() {
                pipe.cv_out.notify_all();
            }
        }
        let mut blocked = true;
        if pos < cur.len() {
            match s.write(&cur[pos..]) {
                Ok(0) => break,
                Ok(n) => {
                    pos += n;
                    blocked = false;
                }
                Err(e) if would_block(&e) => {}
                Err(_) => break,
            }
        }
        if pos == cur.len() {
            // a stack that holds back part of a record sends it here
            match s.flush() {
                Ok(()) => {}
                Err(e) if would_block(&e) => {}
                Err(_) => break,
            }
            // more queued by now: write it before waiting in a read
            blocked = pipe.st.lock().unwrap().out.is_empty();
        }
        // read when the write would block (drains the peer) or there is nothing to write
        if blocked {
            #[cfg(unix)]
            if let (Some(sock), Some(wake), false) = (&sock, &pipe.wake, full) {
                match wait::wait(sock, wake, pos < cur.len(), WAIT) {
                    wait::Ready::Input => {}
                    wait::Ready::Write | wait::Ready::Timeout => continue,
                }
            }
            #[cfg(not(unix))]
            let _ = (&sock, full);
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    full = n == buf.len();
                    pipe.st.lock().unwrap().inb.extend_from_slice(&buf[..n]);
                    pipe.cv_in.notify_all();
                }
                Err(e) if would_block(&e) => full = false,
                Err(_) => break,
            }
        }
    }
    pipe.close();
    close(&mut s);
}

/// Wraps an authenticated stream (TLS done, any pre-HELLO exchange done) as a carrier: one I/O
/// thread owns it (see the module doc). Set short read/write timeouts on it first.
pub fn carrier<S: Read + Write + Socket + Send + 'static>(s: S, hooks: crate::conn::Hooks, close: impl FnOnce(&mut S) + Send + 'static) -> Carrier {
    let sock = s.tcp();
    #[cfg(unix)]
    let wake = sock.as_ref().and_then(|_| wait::Wake::new());
    #[cfg(not(unix))]
    let wake: Option<wait::Wake> = None;
    let pipe = Arc::new(Pipe { wake, ..Default::default() });
    let p = pipe.clone();
    let _ = std::thread::Builder::new().name("makepad-remote-io".into()).spawn(move || {
        (hooks.enter)();
        io_loop(s, sock, p, close);
        (hooks.exit)();
    });
    let closer = pipe.clone();
    Carrier { reader: FrameReader::new(Box::new(PipeRead(pipe.clone()))), writer: Box::new(PipeWrite(pipe)), shutdown: Box::new(move || closer.close()) }
}

/// Connects to a paired endpoint at `addr` (`host:port`): TLS, pinned certificate, key proof.
pub fn dial(addr: &str, creds: &ClientCredentials) -> io::Result<Carrier> {
    let conn = connect_with_credentials(addr, creds)?;
    conn.set_read_timeout(Some(POLL))?;
    conn.set_write_timeout(Some(POLL))?;
    Ok(carrier(conn, crate::conn::Hooks::default(), |c: &mut TunnelConn| c.shutdown()))
}

/// The accepted keys, asked for on every accept (so a newly paired key works without a restart).
pub type KeySource = Arc<dyn Fn() -> Vec<Psk> + Send + Sync>;

/// Called with the key a peer proved and its connection, once the handshake is done.
pub type Bound = Arc<dyn Fn(&Psk, &crate::conn::Conn) + Send + Sync>;

/// A loopback, private or link-local address (a LAN): what a paired-peer listener may bind.
pub fn is_lan(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_lan(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// The address a paired-peer listener binds: `bind` resolved, refused unless it is a LAN or
/// loopback address (never the wildcard or a public address: a leaked key would expose remote
/// eval / probes of every paired device to the internet).
pub fn lan_bind_addr(bind: &str) -> io::Result<std::net::SocketAddr> {
    let addr = bind.to_socket_addrs()?.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address to bind"))?;
    if addr.ip().is_unspecified() || !is_lan(addr.ip()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("refusing to listen on {}: paired devices dial a LAN (private) or loopback address only", addr.ip()),
        ));
    }
    Ok(addr)
}

/// Accepts paired peers on `bind` (`host:port`, a LAN or loopback address: `lan_bind_addr`): each
/// connection does the TLS handshake with `identity` and must prove one of `keys` before anything
/// else is read. Peers that fail are dropped and logged. Returns the bound address.
pub fn listen(endpoint: &Endpoint, bind: &str, identity: TlsIdentity, keys: KeySource, bound: Option<Bound>) -> io::Result<std::net::SocketAddr> {
    let addr = lan_bind_addr(bind)?;
    let listener = TcpListener::bind(addr)?;
    let local = listener.local_addr()?;
    let fingerprint = identity.fingerprint;
    let tls = Arc::new(TlsServer::new(identity)?);
    let ep = endpoint.clone();
    let hooks = ep.hooks();
    let _ = std::thread::Builder::new().name("makepad-remote-accept".into()).spawn(move || {
        (hooks.enter)();
        for tcp in listener.incoming() {
            let Ok(tcp) = tcp else { continue };
            let (ep, tls, keys, bound) = (ep.clone(), tls.clone(), keys.clone(), bound.clone());
            let _ = std::thread::Builder::new().name("makepad-remote-hello".into()).spawn(move || {
                let hooks = ep.hooks();
                (hooks.enter)();
                let peer = tcp.peer_addr().map(|a| a.to_string()).unwrap_or_default();
                if let Err(e) = accept_one(&ep, &tls, &keys, bound.as_deref(), &fingerprint, tcp) {
                    eprintln!("makepad-remote: tcp peer {peer} dropped: {e}");
                }
                (hooks.exit)();
            });
        }
    });
    Ok(local)
}

fn accept_one(ep: &Endpoint, tls: &TlsServer, keys: &KeySource, bound: Option<&(dyn Fn(&Psk, &crate::conn::Conn) + Send + Sync)>, fingerprint: &[u8; 32], tcp: std::net::TcpStream) -> io::Result<()> {
    tcp.set_read_timeout(Some(HANDSHAKE))?;
    tcp.set_write_timeout(Some(HANDSHAKE))?;
    let mut stream: TlsStream = tls.accept(tcp)?;
    let proved = server_authenticate(&mut stream, &keys(), fingerprint).map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e.to_string()))?;
    stream.set_write_timeout(Some(POLL))?;
    stream.set_read_timeout(Some(POLL))?;
    let c = carrier(stream, ep.hooks(), |s: &mut TlsStream| s.shutdown());
    let conn = ep.serve(c.reader, c.writer, c.shutdown).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    if let Some(b) = bound {
        b(&proved, &conn);
    }
    Ok(())
}
