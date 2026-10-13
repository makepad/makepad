//! A connection over a pipe pair: this process's own stdin/stdout (`ssh host makepad remote-relay`
//! runs the far end), or a child process's.

use super::Carrier;
use crate::endpoint::Endpoint;
use crate::frame::FrameReader;
use std::io;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// The carrier over this process's stdin and stdout.
pub fn own() -> Carrier {
    Carrier { reader: FrameReader::new(Box::new(io::stdin())), writer: Box::new(io::stdout()), shutdown: Box::new(|| {}) }
}

/// Serves `endpoint` over this process's stdin/stdout; returns when the peer is gone.
pub fn serve_own(endpoint: &Endpoint) -> io::Result<()> {
    let c = own();
    let conn = endpoint.serve(c.reader, c.writer, c.shutdown).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    conn.wait_closed();
    Ok(())
}

/// Starts `command` with piped stdin/stdout (stderr is inherited) and runs the carrier over them.
/// Shutting the carrier down kills the child.
pub fn spawn(command: &mut Command) -> io::Result<Carrier> {
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn()?;
    let stdin = child.stdin.take().ok_or_else(|| io::Error::other("no child stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| io::Error::other("no child stdout"))?;
    let child: Arc<Mutex<Child>> = Arc::new(Mutex::new(child));
    Ok(Carrier {
        reader: FrameReader::new(Box::new(stdout)),
        writer: Box::new(stdin),
        shutdown: Box::new(move || {
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }),
    })
}
