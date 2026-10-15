//! The network crate's socket factory on Android: TLS streams come from Java (no libssl for native
//! code), through the HAL's JNI socket calls. Registered once, before anything dials (makepad-remote's
//! tunnel, HTTPS fetches).
use makepad_hal::backend::android::socket;
use makepad_network::{AndroidSocketStream, AndroidSocketStreamFactory};
use std::io;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The host's stream ids are 1, 2, 3 ...; the program's (makepad-platform, hal_socket_* ops) all have
/// bit 62 set, so the two never collide in the Activity's one id table.
static NEXT: AtomicI64 = AtomicI64::new(1);

struct Factory;
struct Stream { id: i64, closed: bool }

fn other(m: &str) -> io::Error { io::Error::new(io::ErrorKind::Other, m.to_string()) }
fn ms(t: Option<Duration>) -> i32 { t.map_or(0, |d| d.as_millis().min(i32::MAX as u128) as i32) }

impl AndroidSocketStreamFactory for Factory {
    fn connect(&self, host: &str, port: &str, use_tls: bool, ignore_ssl_cert: bool) -> io::Result<Box<dyn AndroidSocketStream>> {
        let port: i32 = port.parse().map_err(|_| other("invalid port"))?;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        if !socket::open(id, host, port, use_tls, ignore_ssl_cert) { return Err(other("the Java side could not open the socket stream")); }
        Ok(Box::new(Stream { id, closed: false }))
    }
}

impl Drop for Stream { fn drop(&mut self) { self.shutdown(); } }

impl AndroidSocketStream for Stream {
    fn set_read_timeout(&self, t: Option<Duration>) -> io::Result<()> { socket::set_timeout(self.id, true, ms(t)); Ok(()) }
    fn set_write_timeout(&self, t: Option<Duration>) -> io::Result<()> { socket::set_timeout(self.id, false, ms(t)); Ok(()) }
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() { return Ok(0); }
        match socket::read_into(self.id, buf) {
            Some(0) => Err(io::Error::new(io::ErrorKind::TimedOut, "read timed out")),
            Some(n) => Ok(n),
            None => Ok(0),
        }
    }
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() { return Ok(0); }
        let n = socket::write(self.id, buf);
        if n < 0 { Err(other("socket stream write failed")) } else { Ok(n as usize) }
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
    fn peer_fingerprint(&self) -> Option<[u8; 32]> { socket::peer_fingerprint(self.id)?.try_into().ok() }
    fn shutdown(&mut self) { if !self.closed { socket::close(self.id); self.closed = true; } }
}

pub fn register() { makepad_network::register_android_socket_stream_factory_shim(Arc::new(Factory)); }
