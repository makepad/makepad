//! Carriers: what a connection runs over. Each produces a `FrameReader`, a writer and a shutdown
//! function; the handshake and everything above are the same for all of them.

pub mod loopback;
pub mod stdio;
#[cfg(not(target_arch = "wasm32"))]
pub mod tcp;
pub mod unix;

use crate::frame::FrameReader;
use std::io::Write;

/// The halves of a carrier before the handshake.
pub struct Carrier {
    pub reader: FrameReader,
    pub writer: Box<dyn Write + Send>,
    pub shutdown: Box<dyn Fn() + Send + Sync>,
}
