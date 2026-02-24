use makepad_apple_sys::{
    errSSLClosedAbort, errSSLClosedGraceful, errSSLWouldBlock, kSSLClientSide, kSSLStreamType,
    CFRelease, OSStatus, SSLClose, SSLConnectionRef, SSLContextRef, SSLCreateContext, SSLHandshake,
    SSLRead, SSLSetConnection, SSLSetEnableCertVerify, SSLSetIOFuncs, SSLSetPeerDomainName,
    SSLWrite,
};
use std::{
    io,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    ptr,
    time::Duration,
};

const SSL_OK: OSStatus = 0;
const ERR_SSL_WOULD_BLOCK: OSStatus = errSSLWouldBlock;
const ERR_SSL_CLOSED_GRACEFUL: OSStatus = errSSLClosedGraceful;
const ERR_SSL_CLOSED_ABORT: OSStatus = errSSLClosedAbort;

fn io_other(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg.into())
}

fn check_ssl_status(stage: &str, status: OSStatus) -> io::Result<()> {
    if status == SSL_OK {
        Ok(())
    } else {
        Err(io_other(format!("{stage} failed with status {status}")))
    }
}

unsafe extern "C" fn ssl_read_callback(
    connection: SSLConnectionRef,
    data: *mut std::ffi::c_void,
    data_len: *mut usize,
) -> OSStatus {
    if connection.is_null() || data.is_null() || data_len.is_null() {
        return ERR_SSL_CLOSED_ABORT;
    }

    let stream = &mut *(connection as *mut TcpStream);
    let requested = *data_len;
    if requested == 0 {
        return SSL_OK;
    }

    let buffer = std::slice::from_raw_parts_mut(data as *mut u8, requested);
    match stream.read(buffer) {
        Ok(0) => {
            *data_len = 0;
            ERR_SSL_CLOSED_GRACEFUL
        }
        Ok(read_bytes) => {
            *data_len = read_bytes;
            SSL_OK
        }
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
            ) =>
        {
            *data_len = 0;
            ERR_SSL_WOULD_BLOCK
        }
        Err(_) => {
            *data_len = 0;
            ERR_SSL_CLOSED_ABORT
        }
    }
}

unsafe extern "C" fn ssl_write_callback(
    connection: SSLConnectionRef,
    data: *const std::ffi::c_void,
    data_len: *mut usize,
) -> OSStatus {
    if connection.is_null() || data.is_null() || data_len.is_null() {
        return ERR_SSL_CLOSED_ABORT;
    }

    let stream = &mut *(connection as *mut TcpStream);
    let requested = *data_len;
    if requested == 0 {
        return SSL_OK;
    }

    let buffer = std::slice::from_raw_parts(data as *const u8, requested);
    match stream.write(buffer) {
        Ok(written_bytes) => {
            *data_len = written_bytes;
            SSL_OK
        }
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
            ) =>
        {
            *data_len = 0;
            ERR_SSL_WOULD_BLOCK
        }
        Err(_) => {
            *data_len = 0;
            ERR_SSL_CLOSED_ABORT
        }
    }
}

pub(crate) struct SecureTransportStream {
    tcp_stream: Box<TcpStream>,
    ssl_context: SSLContextRef,
    is_closed: bool,
}

unsafe impl Send for SecureTransportStream {}

impl SecureTransportStream {
    fn connect(tcp_stream: TcpStream, host: &str, verify_peer: bool) -> io::Result<Self> {
        let mut tcp_stream = Box::new(tcp_stream);
        let ssl_context = unsafe { SSLCreateContext(ptr::null(), kSSLClientSide, kSSLStreamType) };
        if ssl_context.is_null() {
            return Err(io_other("SSLCreateContext returned null"));
        }

        if let Err(err) = (|| -> io::Result<()> {
            check_ssl_status("SSLSetIOFuncs", unsafe {
                SSLSetIOFuncs(
                    ssl_context,
                    Some(ssl_read_callback),
                    Some(ssl_write_callback),
                )
            })?;
            check_ssl_status("SSLSetConnection", unsafe {
                SSLSetConnection(
                    ssl_context,
                    tcp_stream.as_mut() as *mut TcpStream as SSLConnectionRef,
                )
            })?;
            check_ssl_status("SSLSetPeerDomainName", unsafe {
                SSLSetPeerDomainName(
                    ssl_context,
                    host.as_ptr() as *const std::ffi::c_void,
                    host.len(),
                )
            })?;
            if !verify_peer {
                check_ssl_status("SSLSetEnableCertVerify", unsafe {
                    SSLSetEnableCertVerify(ssl_context, false)
                })?;
            }

            loop {
                let status = unsafe { SSLHandshake(ssl_context) };
                match status {
                    SSL_OK => break,
                    ERR_SSL_WOULD_BLOCK => continue,
                    _ => {
                        return Err(io_other(format!(
                            "SSLHandshake failed with status {status}"
                        )));
                    }
                }
            }
            Ok(())
        })() {
            unsafe {
                let _ = SSLClose(ssl_context);
                CFRelease(ssl_context);
            }
            return Err(err);
        }

        Ok(Self {
            tcp_stream,
            ssl_context,
            is_closed: false,
        })
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.tcp_stream.set_read_timeout(timeout)
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.tcp_stream.set_write_timeout(timeout)
    }

    fn shutdown(&mut self) {
        if self.is_closed {
            return;
        }
        self.is_closed = true;
        unsafe {
            let _ = SSLClose(self.ssl_context);
            CFRelease(self.ssl_context);
        }
        let _ = self.tcp_stream.shutdown(Shutdown::Both);
    }
}

impl Drop for SecureTransportStream {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Read for SecureTransportStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut processed = 0usize;
        let status = unsafe {
            SSLRead(
                self.ssl_context,
                buf.as_mut_ptr() as *mut std::ffi::c_void,
                buf.len(),
                &mut processed,
            )
        };
        match status {
            SSL_OK => Ok(processed),
            ERR_SSL_WOULD_BLOCK => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "SSLRead would block",
            )),
            ERR_SSL_CLOSED_GRACEFUL => Ok(0),
            ERR_SSL_CLOSED_ABORT => Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "SSLRead connection aborted",
            )),
            _ => Err(io_other(format!("SSLRead failed with status {status}"))),
        }
    }
}

impl Write for SecureTransportStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut processed = 0usize;
        let status = unsafe {
            SSLWrite(
                self.ssl_context,
                buf.as_ptr() as *const std::ffi::c_void,
                buf.len(),
                &mut processed,
            )
        };
        match status {
            SSL_OK => Ok(processed),
            ERR_SSL_WOULD_BLOCK => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "SSLWrite would block",
            )),
            ERR_SSL_CLOSED_ABORT => Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "SSLWrite connection aborted",
            )),
            _ => Err(io_other(format!("SSLWrite failed with status {status}"))),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) enum SocketStream {
    Plain(TcpStream),
    Tls(SecureTransportStream),
}

impl SocketStream {
    pub fn connect(
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
    ) -> io::Result<Self> {
        let tcp_stream = TcpStream::connect(format!("{host}:{port}"))?;
        let _ = tcp_stream.set_nodelay(true);

        if use_tls {
            Ok(SocketStream::Tls(SecureTransportStream::connect(
                tcp_stream,
                host,
                !ignore_ssl_cert,
            )?))
        } else {
            Ok(SocketStream::Plain(tcp_stream))
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            SocketStream::Plain(stream) => stream.set_read_timeout(timeout),
            SocketStream::Tls(stream) => stream.set_read_timeout(timeout),
        }
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            SocketStream::Plain(stream) => stream.set_write_timeout(timeout),
            SocketStream::Tls(stream) => stream.set_write_timeout(timeout),
        }
    }

    pub fn shutdown(&mut self) {
        match self {
            SocketStream::Plain(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
            SocketStream::Tls(stream) => stream.shutdown(),
        }
    }
}

impl Read for SocketStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            SocketStream::Plain(stream) => stream.read(buf),
            SocketStream::Tls(stream) => stream.read(buf),
        }
    }
}

impl Write for SocketStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            SocketStream::Plain(stream) => stream.write(buf),
            SocketStream::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            SocketStream::Plain(stream) => stream.flush(),
            SocketStream::Tls(stream) => stream.flush(),
        }
    }
}
