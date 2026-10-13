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
}

impl Pipe {
    fn close(&self) {
        self.st.lock().unwrap().closed = true;
        self.cv_in.notify_all();
        self.cv_out.notify_all();
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
        st.out.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The I/O thread: owns `s` until the pipe closes or the stream fails; `close` shuts it down.
fn io_loop<S: Read + Write>(mut s: S, pipe: Arc<Pipe>, close: impl FnOnce(&mut S)) {
    let mut cur: Vec<u8> = Vec::new();
    let mut pos = 0;
    let mut buf = vec![0u8; 64 << 10];
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
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    pipe.st.lock().unwrap().inb.extend_from_slice(&buf[..n]);
                    pipe.cv_in.notify_all();
                }
                Err(e) if would_block(&e) => {}
                Err(_) => break,
            }
        }
    }
    pipe.close();
    close(&mut s);
}

fn carrier<S: Read + Write + Send + 'static>(s: S, hooks: crate::conn::Hooks, close: impl FnOnce(&mut S) + Send + 'static) -> Carrier {
    let pipe = Arc::new(Pipe::default());
    let p = pipe.clone();
    let _ = std::thread::Builder::new().name("makepad-remote-io".into()).spawn(move || {
        (hooks.enter)();
        io_loop(s, p, close);
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

/// Accepts paired peers on `bind` (`host:port`): each connection does the TLS handshake with
/// `identity` and must prove one of `keys` before anything else is read. Peers that fail are
/// dropped and logged. Returns the bound address.
pub fn listen(endpoint: &Endpoint, bind: &str, identity: TlsIdentity, keys: KeySource, bound: Option<Bound>) -> io::Result<std::net::SocketAddr> {
    let addr = bind.to_socket_addrs()?.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address to bind"))?;
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
