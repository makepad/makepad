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

    /// TLS without CA validation (self-signed servers); returns the server
    /// leaf certificate DER. Hash it with [`crate::tls::fingerprint`] for a known-hosts check.
    pub fn connect_capture(host: &str, port: &str) -> io::Result<(Self, Vec<u8>)> {
        let (inner, der) = crate::backend::linux::socket_stream::SocketStream::connect_capture(host, port)?;
        Ok((Self { inner }, der))
    }

    /// Like [`Self::connect_capture`], resuming the TLS session of `session_of`, an earlier
    /// captured connection to the same host (an FTPS data channel resumes its control
    /// connection's session; servers may refuse a data channel that does not).
    pub fn connect_capture_resuming(host: &str, port: &str, session_of: &Self) -> io::Result<(Self, Vec<u8>)> {
        let (inner, der) = crate::backend::linux::socket_stream::SocketStream::connect_capture_resuming(host, port, &session_of.inner)?;
        Ok((Self { inner }, der))
    }

    /// TLS without CA validation. Returns peer certificates as DER, leaf first.
    /// The caller must authenticate them before sending application data.
    pub fn connect_capture_chain(host: &str, port: &str) -> io::Result<(Self, Vec<Vec<u8>>)> {
        let (inner, chain) = crate::backend::linux::socket_stream::SocketStream::connect_capture_chain(host, port)?;
        Ok((Self { inner }, chain))
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
pub(crate) mod apple_impl;

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

    /// TLS without CA validation (self-signed servers); returns the server
    /// leaf certificate DER. Hash it with [`crate::tls::fingerprint`] for a known-hosts check.
    pub fn connect_capture(host: &str, port: &str) -> io::Result<(Self, Vec<u8>)> {
        let (inner, der) = apple_impl::SocketStream::connect_capture(host, port)?;
        Ok((Self { inner }, der))
    }

    /// Like [`Self::connect_capture`], resuming the TLS session of `session_of`, an earlier
    /// captured connection to the same host (an FTPS data channel resumes its control
    /// connection's session; servers may refuse a data channel that does not).
    pub fn connect_capture_resuming(host: &str, port: &str, session_of: &Self) -> io::Result<(Self, Vec<u8>)> {
        let (inner, der) = apple_impl::SocketStream::connect_capture_resuming(host, port, &session_of.inner)?;
        Ok((Self { inner }, der))
    }

    /// TLS without CA validation. Returns peer certificates as DER, leaf first.
    /// The caller must authenticate them before sending application data.
    pub fn connect_capture_chain(host: &str, port: &str) -> io::Result<(Self, Vec<Vec<u8>>)> {
        let (inner, chain) = apple_impl::SocketStream::connect_capture_chain(host, port)?;
        Ok((Self { inner }, chain))
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
pub(crate) mod windows_impl;

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

    /// TLS without CA validation (self-signed servers); returns the server
    /// leaf certificate DER. Hash it with [`crate::tls::fingerprint`] for a known-hosts check.
    pub fn connect_capture(host: &str, port: &str) -> io::Result<(Self, Vec<u8>)> {
        let (inner, der) = windows_impl::SocketStream::connect_capture(host, port)?;
        Ok((Self { inner }, der))
    }

    /// Like [`Self::connect_capture`], resuming the TLS session of `session_of`, an earlier
    /// captured connection to the same host (an FTPS data channel resumes its control
    /// connection's session; servers may refuse a data channel that does not).
    pub fn connect_capture_resuming(host: &str, port: &str, session_of: &Self) -> io::Result<(Self, Vec<u8>)> {
        let (inner, der) = windows_impl::SocketStream::connect_capture_resuming(host, port, &session_of.inner)?;
        Ok((Self { inner }, der))
    }

    /// TLS without CA validation. Returns peer certificates as DER, leaf first.
    /// The caller must authenticate them before sending application data.
    pub fn connect_capture_chain(host: &str, port: &str) -> io::Result<(Self, Vec<Vec<u8>>)> {
        let (inner, chain) = windows_impl::SocketStream::connect_capture_chain(host, port)?;
        Ok((Self { inner }, chain))
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

    /// TLS without CA validation (self-signed servers); returns the server
    /// leaf certificate DER. Hash it with [`crate::tls::fingerprint`] for a known-hosts check.
    pub fn connect_capture(host: &str, port: &str) -> io::Result<(Self, Vec<u8>)> {
        let inner = crate::backend::connect_platform_socket_stream(host, port, true, true)?;
        let der = inner.peer_certificate().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "the TLS server presented no certificate")
        })?;
        Ok((Self { inner }, der))
    }

    pub fn connect_capture_resuming(_host: &str, _port: &str, _session_of: &Self) -> io::Result<(Self, Vec<u8>)> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "TLS session resumption is not available on this target",
        ))
    }

    pub fn connect_capture_chain(_host: &str, _port: &str) -> io::Result<(Self, Vec<Vec<u8>>)> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "TLS certificate chain capture is not available on this target"))
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

    pub fn connect_capture(_host: &str, _port: &str) -> io::Result<(Self, Vec<u8>)> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "self-signed TLS is not available on this target",
        ))
    }

    pub fn connect_capture_resuming(_host: &str, _port: &str, _session_of: &Self) -> io::Result<(Self, Vec<u8>)> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "self-signed TLS is not available on this target",
        ))
    }

    pub fn connect_capture_chain(_host: &str, _port: &str) -> io::Result<(Self, Vec<Vec<u8>>)> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "TLS certificate chain capture is not available on this target"))
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
