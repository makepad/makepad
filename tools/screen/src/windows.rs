use crate::protocol::{random_token, SessionLocation};
pub use crate::windows_pty::Pty;
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    time::{Duration, Instant},
};

fn proof(token: &str, challenge: &[u8], role: &[u8]) -> [u8; 32] {
    // HMAC-SHA256 authenticates both directions without transmitting the
    // private endpoint secret to a stale/replaced loopback port.
    let mut inner = vec![0x36u8; 64];
    let mut outer = vec![0x5cu8; 64];
    for (i, byte) in token.bytes().enumerate() {
        inner[i] ^= byte;
        outer[i] ^= byte;
    }
    inner.extend_from_slice(challenge);
    inner.extend_from_slice(role);
    outer.extend_from_slice(&crate::digest::sha256(&inner));
    crate::digest::sha256(&outer)
}
fn equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |different, (a, b)| different | (a ^ b))
            == 0
}
struct Authentication {
    token: String,
    challenge: [u8; 32],
    received: usize,
    response: [u8; 32],
    sent: usize,
    client: [u8; 32],
    client_received: usize,
    started: Instant,
}
pub struct Stream {
    inner: TcpStream,
    authentication: Option<Authentication>,
}
impl Stream {
    pub fn set_nonblocking(&self, value: bool) -> io::Result<()> {
        self.inner.set_nonblocking(value)
    }
    fn authenticate(&mut self) -> io::Result<()> {
        let Some(auth) = &mut self.authentication else {
            return Ok(());
        };
        if auth.started.elapsed() > Duration::from_secs(3) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Screen authentication timed out",
            ));
        }
        while auth.received < 32 {
            let n = self.inner.read(&mut auth.challenge[auth.received..])?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Screen authentication closed",
                ));
            }
            auth.received += n;
        }
        if auth.sent == 0 {
            auth.response = proof(&auth.token, &auth.challenge, b"server");
        }
        while auth.sent < 32 {
            let n = self.inner.write(&auth.response[auth.sent..])?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Screen authentication closed",
                ));
            }
            auth.sent += n;
        }
        while auth.client_received < 32 {
            let n = self.inner.read(&mut auth.client[auth.client_received..])?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Screen authentication closed",
                ));
            }
            auth.client_received += n;
        }
        if !equal(
            &auth.client,
            &proof(&auth.token, &auth.challenge, b"client"),
        ) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Screen authentication rejected",
            ));
        }
        self.authentication = None;
        Ok(())
    }
}
impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.authenticate()?;
        self.inner.read(bytes)
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.authenticate()?;
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.authenticate()?;
        self.inner.flush()
    }
}
pub struct Listener {
    inner: TcpListener,
    token: String,
}
impl Listener {
    pub fn bind() -> io::Result<Self> {
        let inner = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        inner.set_nonblocking(true)?;
        Ok(Self {
            inner,
            token: random_token().map_err(io::Error::other)?,
        })
    }
    pub fn port(&self) -> io::Result<u16> {
        Ok(self.inner.local_addr()?.port())
    }
    pub fn token(&self) -> &str {
        &self.token
    }
    pub fn accept(&self) -> io::Result<(Stream, SocketAddr)> {
        let (inner, address) = self.inner.accept()?;
        if !address.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Screen only accepts loopback clients",
            ));
        }
        inner.set_nonblocking(true)?;
        inner.set_nodelay(true)?;
        Ok((
            Stream {
                inner,
                authentication: Some(Authentication {
                    token: self.token.clone(),
                    challenge: [0; 32],
                    received: 0,
                    response: [0; 32],
                    sent: 0,
                    client: [0; 32],
                    client_received: 0,
                    started: Instant::now(),
                }),
            },
            address,
        ))
    }
}
pub fn connect(location: &SessionLocation, timeout: Duration) -> io::Result<Stream> {
    let deadline = Instant::now() + timeout;
    let endpoint = location.endpoint().map_err(io::Error::other)?;
    let port = endpoint
        .get("port")
        .and_then(makepad_strict_json::Value::as_u64)
        .ok_or_else(|| io::Error::other("Invalid screen port"))? as u16;
    let token = endpoint
        .get("token")
        .and_then(makepad_strict_json::Value::as_str)
        .ok_or_else(|| io::Error::other("Invalid screen authentication"))?;
    let mut stream =
        TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), timeout)?;
    stream.set_nonblocking(true)?;
    stream.set_nodelay(true)?;
    let nonce = random_token().map_err(io::Error::other)?;
    let challenge = crate::digest::sha256(nonce.as_bytes());
    let mut sent = 0;
    let mut response = [0u8; 32];
    let mut received = 0;
    let mut client_sent = 0;
    let client = proof(token, &challenge, b"client");
    while received < 32 || client_sent < 32 {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Screen authentication timed out",
            ));
        }
        let progress = (|| -> io::Result<()> {
            if sent < 32 {
                let n = stream.write(&challenge[sent..])?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "Screen authentication closed",
                    ));
                }
                sent += n;
            }
            if sent == 32 && received < 32 {
                let n = stream.read(&mut response[received..])?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Screen authentication closed",
                    ));
                }
                received += n;
            }
            if received == 32 {
                if !equal(&response, &proof(token, &challenge, b"server")) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Screen server proof differs",
                    ));
                }
                let n = stream.write(&client[client_sent..])?;
                if n == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "Screen authentication closed",
                    ));
                }
                client_sent += n;
            }
            Ok(())
        })();
        match progress {
            Ok(()) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(Stream {
        inner: stream,
        authentication: None,
    })
}
// The endpoint proof is the Windows transport identity boundary; the shared
// reactor checks these sentinels before reading the authenticated stream.
pub fn current_uid() -> u32 {
    0
}
pub fn peer_uid(_stream: &Stream) -> io::Result<u32> {
    Ok(0)
}
static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
unsafe extern "system" fn interrupted(_signal: u32) -> i32 {
    INTERRUPTED.store(true, std::sync::atomic::Ordering::Relaxed);
    1
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetConsoleCtrlHandler(
        handler: Option<unsafe extern "system" fn(u32) -> i32>,
        add: i32,
    ) -> i32;
}
pub struct SignalGuard;
impl SignalGuard {
    pub fn new() -> io::Result<Self> {
        if unsafe { SetConsoleCtrlHandler(Some(interrupted), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self)
    }
    pub fn interrupted(&self) -> Option<i32> {
        INTERRUPTED
            .load(std::sync::atomic::Ordering::Relaxed)
            .then_some(1)
    }
}
impl Drop for SignalGuard {
    fn drop(&mut self) {
        unsafe {
            SetConsoleCtrlHandler(Some(interrupted), 0);
        }
    }
}
