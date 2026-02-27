use std::io::{self, Read, Write};
use std::time::Duration;

#[cfg(target_os = "linux")]
pub struct SocketStream {
    inner: crate::backend::linux::socket_stream::SocketStream,
}

#[cfg(target_os = "linux")]
impl SocketStream {
    pub fn connect(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
    ) -> io::Result<Self> {
        Ok(Self {
            inner: crate::backend::linux::socket_stream::SocketStream::connect(
                host,
                port,
                use_tls,
                ignore_ssl_cert,
            )?,
        })
    }

    pub fn into_tls(self, host: &str, ignore_ssl_cert: bool) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.into_tls(host, ignore_ssl_cert)?,
        })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_write_timeout(timeout)
    }

    pub fn shutdown(&mut self) {
        self.inner.shutdown();
    }
}

#[cfg(target_os = "linux")]
impl Read for SocketStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

#[cfg(target_os = "linux")]
impl Write for SocketStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
#[path = "backend/apple/socket_stream.rs"]
mod apple_impl;

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
pub struct SocketStream {
    inner: apple_impl::SocketStream,
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
impl SocketStream {
    pub fn connect(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
    ) -> io::Result<Self> {
        Ok(Self {
            inner: apple_impl::SocketStream::connect(host, port, use_tls, ignore_ssl_cert)?,
        })
    }

    pub fn into_tls(self, host: &str, ignore_ssl_cert: bool) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.into_tls(host, ignore_ssl_cert)?,
        })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_write_timeout(timeout)
    }

    pub fn shutdown(&mut self) {
        self.inner.shutdown();
    }
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
impl Read for SocketStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
impl Write for SocketStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(target_os = "windows")]
#[path = "backend/windows/socket_stream.rs"]
mod windows_impl;

#[cfg(target_os = "windows")]
pub struct SocketStream {
    inner: windows_impl::SocketStream,
}

#[cfg(target_os = "windows")]
impl SocketStream {
    pub fn connect(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
    ) -> io::Result<Self> {
        Ok(Self {
            inner: windows_impl::SocketStream::connect(host, port, use_tls, ignore_ssl_cert)?,
        })
    }

    pub fn into_tls(self, host: &str, ignore_ssl_cert: bool) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.into_tls(host, ignore_ssl_cert)?,
        })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_write_timeout(timeout)
    }

    pub fn shutdown(&mut self) {
        self.inner.shutdown();
    }
}

#[cfg(target_os = "windows")]
impl Read for SocketStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

#[cfg(target_os = "windows")]
impl Write for SocketStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(target_os = "android")]
pub struct SocketStream {
    inner: Box<dyn crate::backend::PlatformSocketStream>,
}

#[cfg(target_os = "android")]
impl SocketStream {
    pub fn connect(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
    ) -> io::Result<Self> {
        Ok(Self {
            inner: crate::backend::connect_platform_socket_stream(
                host,
                port,
                use_tls,
                ignore_ssl_cert,
            )?,
        })
    }

    pub fn into_tls(self, _host: &str, _ignore_ssl_cert: bool) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "socket stream TLS upgrade is not available on this target",
        ))
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_write_timeout(timeout)
    }

    pub fn shutdown(&mut self) {
        self.inner.shutdown();
    }
}

#[cfg(target_os = "android")]
impl Read for SocketStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

#[cfg(target_os = "android")]
impl Write for SocketStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(target_arch = "wasm32")]
pub struct SocketStream;

#[cfg(target_arch = "wasm32")]
impl SocketStream {
    pub fn connect(
        _host: &str,
        _port: &str,
        _use_tls: bool,
        _ignore_ssl_cert: bool,
    ) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "socket stream API is not available on this target",
        ))
    }

    pub fn into_tls(self, _host: &str, _ignore_ssl_cert: bool) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "socket stream TLS upgrade is not available on this target",
        ))
    }

    pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    pub fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    pub fn shutdown(&mut self) {}
}

#[cfg(target_arch = "wasm32")]
impl Read for SocketStream {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "socket stream API is not available on this target",
        ))
    }
}

#[cfg(target_arch = "wasm32")]
impl Write for SocketStream {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "socket stream API is not available on this target",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
