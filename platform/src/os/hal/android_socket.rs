//! The program's socket factory on Android. The app's makepad-network is compiled inside the
//! program, so its factory slot is the program's own; the TLS streams live in the host (Java), reached
//! through the `hal_socket_*` ops. Registered once at HAL client init, before anything can dial.
//!
//! Stream ids: the host's own streams (its remote tunnel) count 1, 2, 3 ...; the program's all have
//! bit 62 set, so the two never collide in the Activity's one id table.
use makepad_hal::{SOCKET_IGNORE_CERT, SOCKET_TLS};
use makepad_network::{AndroidSocketStream, AndroidSocketStreamFactory};
use std::io;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const PROGRAM_ID_BIT: i64 = 1 << 62;
static NEXT: AtomicI64 = AtomicI64::new(1);

struct Factory;
struct Stream {
    id: i64,
    closed: bool,
}

fn other(m: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, m.to_string())
}

fn ms(t: Option<Duration>) -> i32 {
    t.map_or(0, |d| d.as_millis().min(i32::MAX as u128) as i32)
}

impl AndroidSocketStreamFactory for Factory {
    fn connect(&self, host: &str, port: &str, use_tls: bool, ignore_ssl_cert: bool) -> io::Result<Box<dyn AndroidSocketStream>> {
        let port: i32 = port.parse().map_err(|_| other("invalid port"))?;
        let id = PROGRAM_ID_BIT | NEXT.fetch_add(1, Ordering::Relaxed);
        let flags = if use_tls { SOCKET_TLS } else { 0 } | if ignore_ssl_cert { SOCKET_IGNORE_CERT } else { 0 };
        if makepad_hal::hal_socket_open(id, host, port, flags) == 0 {
            return Err(other("the Java side could not open the socket stream"));
        }
        Ok(Box::new(Stream { id, closed: false }))
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl AndroidSocketStream for Stream {
    fn set_read_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        makepad_hal::hal_socket_timeout(self.id, 1, ms(t));
        Ok(())
    }
    fn set_write_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        makepad_hal::hal_socket_timeout(self.id, 0, ms(t));
        Ok(())
    }
    /// 0 bytes from the host is a read timeout; -1 is a closed stream (end of file, Ok(0)).
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        match makepad_hal::hal_socket_read(self.id, buf) {
            n if n < 0 => Ok(0),
            0 => Err(io::Error::new(io::ErrorKind::TimedOut, "read timed out")),
            n => Ok(n as usize),
        }
    }
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let n = makepad_hal::hal_socket_write(self.id, buf);
        if n < 0 {
            Err(other("socket stream write failed"))
        } else {
            Ok(n as usize)
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn peer_fingerprint(&self) -> Option<[u8; 32]> {
        let mut fp = [0u8; 32];
        (makepad_hal::hal_socket_fingerprint(self.id, &mut fp) == 32).then_some(fp)
    }
    fn shutdown(&mut self) {
        if !self.closed {
            makepad_hal::hal_socket_close(self.id);
            self.closed = true;
        }
    }
}

pub(crate) fn register() {
    makepad_network::register_android_socket_stream_factory_shim(Arc::new(Factory));
}
