//! The local carrier where there is no Unix socket (Windows): TCP on 127.0.0.1 with an ephemeral
//! port, guarded by a random 256-bit token. The endpoint writes `"socket":"tcp:127.0.0.1:PORT"`
//! and `"token":"<hex>"` into its discovery file (in the user's temp folder, readable by that
//! user); a client sends the token in HELLO and the endpoint refuses a HELLO without it. Nothing
//! else about the connection differs from the Unix carrier, but that it gives no peer
//! credentials: a client of the endpoint's own process says so with the process's own secret
//! (`Hello::own`), which it sends only to an address this process serves.

use super::Carrier;
use crate::endpoint::Endpoint;
use crate::frame::FrameReader;
use makepad_network::clone_tcp;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

/// This process's own secret (`Hello::own`): made with its first loopback endpoint, never written
/// anywhere and never sent to another process's endpoint.
static OWN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
/// The addresses this process's loopback endpoints listen on.
static SERVED: std::sync::Mutex<Vec<SocketAddr>> = std::sync::Mutex::new(Vec::new());

/// What the HELLO of a client that dials `addr` says of its process: this process's secret to an
/// endpoint of this process, nothing to any other.
pub fn own_secret_for(addr: &str) -> String {
    let ours = addr.parse::<SocketAddr>().is_ok_and(|a| SERVED.lock().unwrap().contains(&a));
    match OWN.get() {
        Some(own) if ours => own.clone(),
        _ => String::new(),
    }
}

/// A peer whose HELLO carries this process's secret is this process.
pub(crate) fn is_own(claim: &str) -> bool {
    !claim.is_empty() && OWN.get().is_some_and(|own| own == claim)
}

/// 32 random bytes as hex.
pub fn new_token() -> io::Result<String> {
    let mut b = [0u8; 32];
    makepad_network::tls::os_random(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn carrier(s: TcpStream) -> io::Result<Carrier> {
    let _ = s.set_nodelay(true);
    let r = clone_tcp(&s)?;
    let w = clone_tcp(&s)?;
    Ok(Carrier {
        reader: FrameReader::new(Box::new(r)),
        writer: Box::new(w),
        shutdown: Box::new(move || {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }),
    })
}

/// Connects to the endpoint at `addr` (`127.0.0.1:PORT`). The read timeout is for the handshake:
/// call [`settle`] on the returned stream handle afterwards.
pub fn dial(addr: &str) -> io::Result<(Carrier, TcpStream)> {
    let target: SocketAddr = addr.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("bad loopback address {addr}")))?;
    if !target.ip().is_loopback() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "the local carrier only dials loopback addresses"));
    }
    let s = TcpStream::connect_timeout(&target, Duration::from_secs(2))?;
    s.set_read_timeout(Some(Duration::from_secs(1)))?;
    let keep = clone_tcp(&s)?;
    Ok((carrier(s)?, keep))
}

/// Ends the handshake read timeout.
pub fn settle(s: &TcpStream) {
    let _ = s.set_read_timeout(None);
}

/// Listens on 127.0.0.1 (ephemeral port) and serves `ep` to peers that present `token`; returns
/// the bound address. The accept loop runs on its own thread.
pub fn serve(ep: &Endpoint, token: &str) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    if OWN.get().is_none() {
        // (two first endpoints at once: one secret is kept, either serves)
        let _ = OWN.set(new_token()?);
    }
    SERVED.lock().unwrap().push(addr);
    let (ep, token) = (ep.clone(), token.to_string());
    let hooks = ep.hooks();
    std::thread::Builder::new().name("makepad-remote-accept".into()).spawn(move || {
        (hooks.enter)();
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let (ep, token) = (ep.clone(), token.clone());
            let _ = std::thread::Builder::new().name("makepad-remote-hello".into()).spawn(move || {
                let (hooks, keep) = (ep.hooks(), clone_tcp(&conn));
                (hooks.enter)();
                let _ = conn.set_read_timeout(Some(Duration::from_secs(1)));
                if let Ok(c) = carrier(conn) {
                    match ep.serve_checked(c.reader, c.writer, c.shutdown, &token) {
                        Err(e) => eprintln!("makepad-remote: a client was refused: {e}"),
                        Ok(_) => {
                            if let Ok(k) = keep {
                                settle(&k);
                            }
                        }
                    }
                }
                (hooks.exit)();
            });
        }
    })?;
    Ok(addr)
}
