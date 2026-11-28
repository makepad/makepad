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
                None,
            )?,
        })
    }

    /// Each resolved address gets `timeout` to connect. TLS uses the time left since the call began.
    pub fn connect_timeout(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
        timeout: Duration,
    ) -> io::Result<Self> {
        Ok(Self {
            inner: crate::backend::linux::socket_stream::SocketStream::connect(
                host, port, use_tls, ignore_ssl_cert, Some(timeout),
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
            inner: self.inner.into_tls(host, ignore_ssl_cert, None)?,
        })
    }

    /// Bound the wait for TLS handshake input, preserving the read timeout on success.
    pub fn into_tls_timeout(
        self,
        host: &str,
        ignore_ssl_cert: bool,
        timeout: Duration,
    ) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.into_tls(host, ignore_ssl_cert, Some(timeout))?,
        })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_write_timeout(timeout)
    }

    /// A handle on the TCP socket under the stream (to wait for input on it).
    pub fn try_clone_tcp(&self) -> io::Result<std::net::TcpStream> {
        self.inner.try_clone_tcp()
    }

    /// Plaintext the TLS layer holds already: a read returns it without
    /// waiting on the socket. A reader that waits for input on the socket
    /// (`try_clone_tcp`, `peek`) reads first while this is above zero.
    pub fn buffered(&self) -> usize {
        self.inner.buffered()
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
            inner: apple_impl::SocketStream::connect(host, port, use_tls, ignore_ssl_cert, None)?,
        })
    }

    /// Each resolved address gets `timeout` to connect. TLS uses the time left since the call began.
    pub fn connect_timeout(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
        timeout: Duration,
    ) -> io::Result<Self> {
        Ok(Self {
            inner: apple_impl::SocketStream::connect(
                host, port, use_tls, ignore_ssl_cert, Some(timeout),
            )?,
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
            inner: self.inner.into_tls(host, ignore_ssl_cert, None)?,
        })
    }

    /// Bound the wait for TLS handshake input, preserving the read timeout on success.
    pub fn into_tls_timeout(
        self,
        host: &str,
        ignore_ssl_cert: bool,
        timeout: Duration,
    ) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.into_tls(host, ignore_ssl_cert, Some(timeout))?,
        })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_write_timeout(timeout)
    }

    /// A handle on the TCP socket under the stream (to wait for input on it).
    pub fn try_clone_tcp(&self) -> io::Result<std::net::TcpStream> {
        self.inner.try_clone_tcp()
    }

    /// Plaintext the TLS layer holds already: a read returns it without
    /// waiting on the socket. A reader that waits for input on the socket
    /// (`try_clone_tcp`, `peek`) reads first while this is above zero.
    pub fn buffered(&self) -> usize {
        self.inner.buffered()
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
            inner: windows_impl::SocketStream::connect(host, port, use_tls, ignore_ssl_cert, None)?,
        })
    }

    /// Wait up to `timeout` for the platform's connect operation, including TLS.
    pub fn connect_timeout(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
        timeout: Duration,
    ) -> io::Result<Self> {
        Ok(Self {
            inner: windows_impl::SocketStream::connect(
                host, port, use_tls, ignore_ssl_cert, Some(timeout),
            )?,
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

    /// TLS upgrade on an existing socket is unsupported on this target.
    pub fn into_tls_timeout(
        self,
        host: &str,
        ignore_ssl_cert: bool,
        _timeout: Duration,
    ) -> io::Result<Self> {
        self.into_tls(host, ignore_ssl_cert)
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_write_timeout(timeout)
    }

    /// A handle on the TCP socket under the stream (to wait for input on it):
    /// a pinned (Schannel) connection's; a WinRT socket has none.
    pub fn try_clone_tcp(&self) -> io::Result<std::net::TcpStream> {
        self.inner.try_clone_tcp()
    }

    /// Plaintext the TLS layer holds already: a read returns it without
    /// waiting on the socket. A reader that waits for input on the socket
    /// (`try_clone_tcp`, `peek`) reads first while this is above zero.
    pub fn buffered(&self) -> usize {
        self.inner.buffered()
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

    /// Android delegates to the platform connector, which ignores the supplied timeout.
    pub fn connect_timeout(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
        _timeout: Duration,
    ) -> io::Result<Self> {
        Self::connect(host, port, use_tls, ignore_ssl_cert)
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

    /// TLS upgrade on an existing socket is unsupported on this target.
    pub fn into_tls_timeout(
        self,
        host: &str,
        ignore_ssl_cert: bool,
        _timeout: Duration,
    ) -> io::Result<Self> {
        self.into_tls(host, ignore_ssl_cert)
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_write_timeout(timeout)
    }

    /// A handle on the TCP socket under the stream (to wait for input on it): none here.
    pub fn try_clone_tcp(&self) -> io::Result<std::net::TcpStream> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no TCP socket handle on this platform"))
    }

    /// Plaintext the TLS layer holds already: a read returns it without
    /// waiting on the socket. A reader that waits for input on the socket
    /// (`try_clone_tcp`, `peek`) reads first while this is above zero. None here: no handle to wait on, so a reader only reads.
    pub fn buffered(&self) -> usize {
        0
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

    /// Raw sockets are unavailable on WebAssembly.
    pub fn connect_timeout(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
        _timeout: Duration,
    ) -> io::Result<Self> {
        Self::connect(host, port, use_tls, ignore_ssl_cert)
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

    /// TLS upgrade on an existing socket is unsupported on this target.
    pub fn into_tls_timeout(
        self,
        host: &str,
        ignore_ssl_cert: bool,
        _timeout: Duration,
    ) -> io::Result<Self> {
        self.into_tls(host, ignore_ssl_cert)
    }

    pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    pub fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    /// A handle on the TCP socket under the stream (to wait for input on it): none here.
    pub fn try_clone_tcp(&self) -> io::Result<std::net::TcpStream> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no TCP socket handle on this platform"))
    }

    /// Plaintext the TLS layer holds already: a read returns it without
    /// waiting on the socket. A reader that waits for input on the socket
    /// (`try_clone_tcp`, `peek`) reads first while this is above zero. None here: no handle to wait on, so a reader only reads.
    pub fn buffered(&self) -> usize {
        0
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::time::Instant;

    #[test]
    fn socket_stream_connect_timeout_fails_fast_on_refused_loopback() -> io::Result<()> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        // The client's source port stays allocated and has no listener.
        let client = TcpStream::connect(listener.local_addr()?)?;
        let port = client.local_addr()?.port().to_string();
        let started = Instant::now();
        let err = SocketStream::connect_timeout("127.0.0.1", &port, false, true, Duration::from_secs(3))
            .err().expect("a port without a listener accepted a connection");
        assert_ne!(err.kind(), io::ErrorKind::TimedOut, "connect missed the refusal");
        #[cfg(any(target_os = "linux", target_vendor = "apple"))]
        assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused);
        assert!(started.elapsed() < Duration::from_millis(750), "loopback refusal took {:?}", started.elapsed());
        Ok(())
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn socket_stream_into_tls_timeout_upgrades_after_plaintext_greeting() -> io::Result<()> {
        use crate::tls::{generate_p256, TlsIdentity, TlsServer};
        use std::thread;

        const GREETING: &[u8; 15] = b"MDCSTART<<TLS>>";
        let limit = Duration::from_millis(750);
        let server = TlsServer::new(TlsIdentity::in_memory(generate_p256()?, "localhost", None)?)?;
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port().to_string();
        let mut client = SocketStream::connect_timeout("127.0.0.1", &port, false, true, limit)?;
        let (mut peer, _) = listener.accept()?;
        peer.set_read_timeout(Some(limit))?;
        peer.set_write_timeout(Some(limit))?;
        let echo = thread::spawn(move || -> io::Result<()> {
            peer.write_all(GREETING)?;
            let mut tls = server.accept(peer)?;
            let mut request = [0; 2];
            tls.read_exact(&mut request)?;
            tls.write_all(&request)?;
            tls.flush()
        });

        let previous = Some(Duration::from_millis(250));
        client.set_read_timeout(previous)?;
        let mut greeting = [0; GREETING.len()];
        client.read_exact(&mut greeting)?;
        assert_eq!(&greeting, GREETING);
        let mut tls = client.into_tls_timeout("localhost", true, limit)?;
        assert_eq!(tls.try_clone_tcp()?.read_timeout()?, previous, "upgrade changed the read timeout");
        tls.write_all(b"ok")?;
        tls.flush()?;
        let mut reply = [0; 2];
        tls.read_exact(&mut reply)?;
        assert_eq!(&reply, b"ok");
        echo.join().unwrap()?;

        let client = SocketStream::connect_timeout("127.0.0.1", &port, false, true, limit)?;
        assert_eq!(client.try_clone_tcp()?.read_timeout()?, None);
        let (mut peer, _) = listener.accept()?;
        // Drain the ClientHello without replying. Also release the socket if the client hangs.
        peer.set_read_timeout(Some(limit))?;
        let silent = thread::spawn(move || io::copy(&mut peer, &mut io::sink()));
        let started = Instant::now();
        let timeout = Duration::from_millis(150);
        let result = client.into_tls_timeout("localhost", true, timeout);
        let elapsed = started.elapsed();
        let err = result.err().expect("TLS handshake succeeded against a silent peer");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "silent peer: {err}");
        assert!(elapsed >= timeout, "handshake timed out early after {elapsed:?}");
        assert!(elapsed < limit, "silent peer held the handshake for {elapsed:?}");
        silent.join().unwrap()?;
        Ok(())
    }
}
