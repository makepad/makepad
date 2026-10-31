//! The local carrier where there is no Unix socket (Windows): TCP on 127.0.0.1 with an ephemeral
//! port, guarded by a random 256-bit token. The endpoint writes `"socket":"tcp:127.0.0.1:PORT"`
//! and `"token":"<hex>"` into its discovery file (in the user's temp folder, readable by that
//! user); a client sends the token in HELLO and the endpoint refuses a HELLO without it. Nothing
//! else about the connection differs from the Unix carrier.

use super::Carrier;
use crate::endpoint::Endpoint;
use crate::frame::FrameReader;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

/// 32 random bytes as hex.
pub fn new_token() -> io::Result<String> {
    let mut b = [0u8; 32];
    makepad_network::tls::os_random(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn carrier(s: TcpStream) -> io::Result<Carrier> {
    let _ = s.set_nodelay(true);
    let r = s.try_clone()?;
    let w = s.try_clone()?;
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
    let keep = s.try_clone()?;
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
    let (ep, token) = (ep.clone(), token.to_string());
    let hooks = ep.hooks();
    std::thread::Builder::new().name("makepad-remote-accept".into()).spawn(move || {
        (hooks.enter)();
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let (ep, token) = (ep.clone(), token.clone());
            let _ = std::thread::Builder::new().name("makepad-remote-hello".into()).spawn(move || {
                let (hooks, keep) = (ep.hooks(), conn.try_clone());
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
