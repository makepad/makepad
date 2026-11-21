//! The byte line a device protocol (a bootloader's, for one) runs over: writes, byte reads with a
//! timeout, DTR/RTS, baud, drain, reopen and the clock; `TtyLine` is a local serial port.
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

/// Lead choice: allow ten seconds for a USB serial device to enumerate again after a reset and
/// for its port to open.
pub const CONNECT_WINDOW: std::time::Duration = std::time::Duration::from_secs(10);

/// Lead choice: a port that takes no bytes for one second has a device that is not reading; a
/// write then fails instead of waiting forever.
pub const WRITE_STALL: std::time::Duration = std::time::Duration::from_secs(1);

/// One step of a reset dance run by [`Line::sequence`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Dtr(bool),
    Rts(bool),
    /// A wait on the line's clock ([`Line::sleep`]).
    Wait(std::time::Duration),
    /// [`Line::drain_input`].
    Drain,
}

/// Byte transport and reset controls for a device protocol.
pub trait Line {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()>;

    /// One byte, or None when `timeout` passes first.
    fn read_byte(&mut self, timeout: std::time::Duration) -> std::io::Result<Option<u8>>;

    /// Assert (true) or release (false) DTR. Asserted is the active level (low on
    /// a UART wire); for USB CDC it sets the DTR control-line bit to 1.
    fn set_dtr(&mut self, on: bool) -> std::io::Result<()>;

    /// Assert (true) or release (false) RTS. Asserted is the active level (low on
    /// a UART wire); for USB CDC it sets the RTS control-line bit to 1.
    fn set_rts(&mut self, on: bool) -> std::io::Result<()>;

    fn set_baud(&mut self, baud: u32) -> std::io::Result<()>;

    /// Discard every byte received and not yet read.
    fn drain_input(&mut self) -> std::io::Result<()>;

    /// Run `steps` in order as one unit: a remote line sends them whole and its far end finishes a
    /// started sequence even if the link drops, so no board is left held in reset. Err names the
    /// index of the step that failed; the steps after it did not run.
    fn sequence(&mut self, steps: &[Step]) -> Result<(), (usize, std::io::Error)> {
        for (index, step) in steps.iter().enumerate() {
            let result = match *step {
                Step::Dtr(on) => self.set_dtr(on),
                Step::Rts(on) => self.set_rts(on),
                Step::Wait(d) => {
                    self.sleep(d);
                    Ok(())
                }
                Step::Drain => self.drain_input(),
            };
            result.map_err(|err| (index, err))?;
        }
        Ok(())
    }

    /// Drop the old port and reconnect to the same device at the current baud.
    fn reopen(&mut self) -> std::io::Result<()>;

    /// Waits (a real line sleeps; a test line advances its virtual clock).
    fn sleep(&mut self, d: std::time::Duration);

    /// Monotonic host time. Virtual-time lines override this along with `sleep`
    /// and `read_byte`, so deadlines and byte arrival times use the same clock.
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
}

/// A line that logs every call to stderr: what was sent, what came back and when, so a device
/// that does not answer can be told from one that answers wrongly.
pub struct Traced<L: Line> {
    pub inner: L,
    start: std::time::Instant,
}

impl<L: Line> Traced<L> {
    pub fn new(inner: L) -> Self {
        Self { inner, start: std::time::Instant::now() }
    }

    fn log(&self, what: std::fmt::Arguments) {
        eprintln!("{:>9.3} ms {}", self.start.elapsed().as_secs_f64() * 1e3, what);
    }
}

impl<L: Line> Line for Traced<L> {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.log(format_args!("tx {}", hex(bytes)));
        self.inner.write_all(bytes)
    }

    fn read_byte(&mut self, timeout: std::time::Duration) -> std::io::Result<Option<u8>> {
        let r = self.inner.read_byte(timeout);
        match &r {
            Ok(Some(b)) => self.log(format_args!("rx {:02x}", b)),
            Ok(None) => self.log(format_args!("rx timeout ({} ms)", timeout.as_millis())),
            Err(e) => self.log(format_args!("rx error {}", e)),
        }
        r
    }

    fn set_dtr(&mut self, on: bool) -> std::io::Result<()> {
        self.log(format_args!("dtr {}", on as u8));
        self.inner.set_dtr(on)
    }

    fn set_rts(&mut self, on: bool) -> std::io::Result<()> {
        self.log(format_args!("rts {}", on as u8));
        self.inner.set_rts(on)
    }

    fn set_baud(&mut self, baud: u32) -> std::io::Result<()> {
        self.log(format_args!("baud {}", baud));
        self.inner.set_baud(baud)
    }

    fn drain_input(&mut self) -> std::io::Result<()> {
        self.log(format_args!("drain"));
        self.inner.drain_input()
    }

    fn sequence(&mut self, steps: &[Step]) -> Result<(), (usize, std::io::Error)> {
        self.log(format_args!("sequence {steps:?}"));
        let result = self.inner.sequence(steps);
        if let Err((index, err)) = &result {
            self.log(format_args!("sequence failed at step {index}: {err}"));
        }
        result
    }

    fn reopen(&mut self) -> std::io::Result<()> {
        self.log(format_args!("reopen"));
        self.inner.reopen()
    }

    fn sleep(&mut self, d: std::time::Duration) {
        self.inner.sleep(d)
    }

    fn now(&self) -> std::time::Instant {
        self.inner.now()
    }
}

/// Bytes as space-separated hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (k, b) in bytes.iter().enumerate() {
        if k > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// A local serial port as a line.
pub struct TtyLine {
    port: Option<Box<dyn crate::SerialPort>>,
    path: String,
    baud: u32,
}

impl std::fmt::Debug for TtyLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TtyLine").finish_non_exhaustive()
    }
}

impl TtyLine {
    pub fn open(path: &str, baud: u32) -> io::Result<Self> {
        Ok(Self {
            port: Some(crate::open(path, baud, Duration::MAX)?),
            path: path.to_owned(),
            baud,
        })
    }

    fn port(&mut self) -> io::Result<&mut (dyn crate::SerialPort + 'static)> {
        self.port.as_deref_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "serial port is not open")
        })
    }
}

impl Line for TtyLine {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        // A device that stops reading would otherwise block this write forever.
        self.port()?.set_timeout(WRITE_STALL)?;
        Write::write_all(self.port()?, bytes).map_err(|err| match err.kind() {
            io::ErrorKind::TimedOut => io::Error::new(io::ErrorKind::TimedOut, format!(
                "{} took no data for {} s: the device is not reading", self.path, WRITE_STALL.as_secs())),
            _ => err,
        })
    }

    fn read_byte(&mut self, timeout: Duration) -> io::Result<Option<u8>> {
        self.port()?.set_timeout(timeout)?;
        let mut byte = [0];
        match Read::read(self.port()?, &mut byte) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "serial port closed",
            )),
            Ok(_) => Ok(Some(byte[0])),
            Err(err) if err.kind() == io::ErrorKind::TimedOut => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn set_dtr(&mut self, on: bool) -> io::Result<()> {
        self.port()?.write_data_terminal_ready(on)
    }

    fn set_rts(&mut self, on: bool) -> io::Result<()> {
        self.port()?.write_request_to_send(on)
    }

    fn set_baud(&mut self, baud: u32) -> io::Result<()> {
        self.port()?.set_baud(baud)?;
        self.baud = baud;
        Ok(())
    }

    fn drain_input(&mut self) -> io::Result<()> {
        self.port()?.clear_input()
    }

    fn reopen(&mut self) -> io::Result<()> {
        // Lead choice: retry every 100 ms, reporting once per second, for the connection
        // window.
        const RETRY: Duration = Duration::from_millis(100);
        drop(self.port.take());
        let start = Instant::now();
        let mut next_message = Duration::ZERO;
        loop {
            let err = match crate::open(&self.path, self.baud, Duration::MAX) {
                Ok(port) => { self.port = Some(port); return Ok(()); }
                Err(err) => err,
            };
            let waited = start.elapsed();
            if waited < CONNECT_WINDOW {
                if waited >= next_message {
                    eprintln!("waiting for {}", self.path);
                    next_message = waited + Duration::from_secs(1);
                }
                std::thread::sleep(RETRY.min(CONNECT_WINDOW - waited));
            }
            let waited = start.elapsed();
            if waited >= CONNECT_WINDOW {
                return Err(io::Error::new(err.kind(), format!(
                    "cannot reopen {} after {:.3} s: {err}", self.path, waited.as_secs_f64()
                )));
            }
        }
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}
