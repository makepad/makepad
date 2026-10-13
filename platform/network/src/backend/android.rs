use std::{
    io,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

pub trait PlatformSocketStream: Send {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write(&mut self, buf: &[u8]) -> io::Result<usize>;
    fn flush(&mut self) -> io::Result<()>;
    /// SHA-256 of the TLS server's leaf certificate, once handshaken.
    fn peer_fingerprint(&self) -> Option<[u8; 32]>;
    fn shutdown(&mut self);
}

pub trait PlatformSocketFactory: Send + Sync {
    fn connect(
        &self,
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
    ) -> io::Result<Box<dyn PlatformSocketStream>>;
}

fn socket_factory_slot() -> &'static Mutex<Option<Arc<dyn PlatformSocketFactory>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<dyn PlatformSocketFactory>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub fn register_platform_socket_factory(factory: Arc<dyn PlatformSocketFactory>) {
    if let Ok(mut slot) = socket_factory_slot().lock() {
        *slot = Some(factory);
    }
}

pub fn clear_platform_socket_factory() {
    if let Ok(mut slot) = socket_factory_slot().lock() {
        *slot = None;
    }
}

pub(crate) fn connect_platform_socket_stream(
    host: &str,
    port: &str,
    use_tls: bool,
    ignore_ssl_cert: bool,
) -> io::Result<Box<dyn PlatformSocketStream>> {
    let slot = socket_factory_slot().lock().map_err(|_| {
        io::Error::new(
            io::ErrorKind::Other,
            "android socket stream factory lock poisoned",
        )
    })?;

    let factory = slot.as_ref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "android socket stream shim not registered by makepad-platform",
        )
    })?;

    factory.connect(host, port, use_tls, ignore_ssl_cert)
}
