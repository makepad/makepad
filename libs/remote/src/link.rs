//! The blocking client: dials an endpoint (Unix socket, TCP, or a child process's pipes), runs
//! `cmd` requests, opens other services and follows `watch` / `stream` feeds. When the link is
//! lost the next call reconnects and sends RESUME with the last sequence number it saw per feed;
//! a request that was in flight when the link dropped returns [`Error::LinkLost`].

use crate::carrier::{self, Carrier};
use crate::conn::{Channel, Conn, Event, Hooks, Io, CLOSE_LINK_LOST, CLOSE_NORMAL};
use crate::hello::{self, Hello, Resume};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug)]
pub enum Error {
    /// The link dropped (or the carrier could not be reached); the next call reconnects.
    LinkLost(String),
    /// The endpoint refused: close code and reason (unsupported service, version mismatch, ...).
    Refused(u16, String),
    /// The answer did not finish within the client's timeout; the request's channel was closed
    /// and the connection stays usable.
    Timeout,
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::LinkLost(w) => write!(f, "link lost: {w}"),
            Error::Refused(_, w) => f.write_str(w),
            Error::Timeout => f.write_str("no answer within the timeout"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// Where a client dials.
#[derive(Clone)]
pub enum Target {
    Unix(PathBuf),
    /// `127.0.0.1:PORT` and the token of the discovery file (the local carrier where there are no
    /// Unix sockets)
    #[cfg(not(target_arch = "wasm32"))]
    Loopback(String, String),
    /// `host:port` and the credentials of the pairing
    #[cfg(not(target_arch = "wasm32"))]
    Tcp(String, Arc<makepad_network::tunnel::ClientCredentials>),
    /// a command whose stdin/stdout speak makepad-remote (`ssh host makepad remote-relay`)
    Command(Vec<String>),
    /// an endpoint behind a hub (`<host>:<app|pid>`): every channel is the hub's `route` to it
    Routed(Client, String),
}

struct Inner {
    target: Target,
    name: String,
    link_id: String,
    conn: Mutex<Option<Conn>>,
    ever: Mutex<bool>,
    seqs: Mutex<HashMap<String, u64>>,
    caps: Mutex<Vec<String>>,
    timeout: Mutex<Option<Duration>>,
}

#[derive(Clone)]
pub struct Client(Arc<Inner>);

fn link_id() -> String {
    format!("{}-{:x}", std::process::id(), crate::cmd::wall_ms())
}

impl Client {
    pub fn new(target: Target, name: &str) -> Client {
        Client(Arc::new(Inner { target, name: name.to_string(), link_id: link_id(), conn: Mutex::new(None), ever: Mutex::new(false), seqs: Mutex::new(HashMap::new()), caps: Mutex::new(Vec::new()), timeout: Mutex::new(None) }))
    }

    /// Dials a local endpoint: `spec` is a socket path, a pid or an app name (discovery files).
    pub fn connect(spec: &str) -> Result<Client, Error> {
        let target = if spec.contains('/') || spec.ends_with(".sock") {
            Target::Unix(PathBuf::from(spec))
        } else {
            let app = crate::discovery::find(spec).ok_or_else(|| Error::Refused(CLOSE_LINK_LOST, format!("no running makepad-remote endpoint `{spec}`")))?;
            match app.socket.strip_prefix("tcp:") {
                Some(addr) => Target::Loopback(addr.to_string(), app.text("token").unwrap_or_default()),
                None => Target::Unix(PathBuf::from(app.socket)),
            }
        };
        let c = Client::new(target, "client");
        c.conn()?;
        Ok(c)
    }

    /// Dials a paired endpoint over TCP+TLS.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect_tcp(addr: &str, creds: makepad_network::tunnel::ClientCredentials) -> Result<Client, Error> {
        let c = Client::new(Target::Tcp(addr.to_string(), Arc::new(creds)), "client");
        c.conn()?;
        Ok(c)
    }

    /// Runs `command` and speaks makepad-remote over its stdin/stdout.
    pub fn connect_command(command: Vec<String>) -> Result<Client, Error> {
        let c = Client::new(Target::Command(command), "client");
        c.conn()?;
        Ok(c)
    }

    fn carrier(&self) -> io::Result<(Carrier, Option<Box<dyn FnOnce() + Send>>)> {
        match &self.0.target {
            Target::Unix(p) => {
                #[cfg(unix)]
                {
                    let (c, keep) = carrier::unix::dial(p)?;
                    Ok((c, Some(Box::new(move || carrier::unix::settle(&keep)))))
                }
                #[cfg(not(unix))]
                {
                    let _ = p;
                    Err(io::Error::new(io::ErrorKind::Unsupported, "makepad-remote is not available on Windows yet"))
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            Target::Loopback(addr, _) => {
                let (c, keep) = carrier::loopback::dial(addr)?;
                Ok((c, Some(Box::new(move || carrier::loopback::settle(&keep)))))
            }
            #[cfg(not(target_arch = "wasm32"))]
            Target::Tcp(addr, creds) => Ok((carrier::tcp::dial(addr, creds)?, None)),
            Target::Routed(..) => Err(io::Error::new(io::ErrorKind::Unsupported, "a routed client opens its channels through the hub")),
            Target::Command(argv) => {
                let (first, rest) = argv.split_first().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
                Ok((carrier::stdio::spawn(std::process::Command::new(first).args(rest))?, None))
            }
        }
    }

    /// The live connection, dialling (and resuming) when there is none.
    pub fn conn(&self) -> Result<Conn, Error> {
        let mut slot = self.0.conn.lock().unwrap();
        if let Some(c) = slot.as_ref() {
            if c.is_usable() {
                return Ok(c.clone());
            }
        }
        let (mut car, settle) = self.carrier().map_err(|e| Error::LinkLost(e.to_string()))?;
        car.reader.deadline = Some(std::time::Instant::now() + Duration::from_secs(10));
        let token = match &self.0.target {
            #[cfg(not(target_arch = "wasm32"))]
            Target::Loopback(_, token) => token.clone(),
            _ => String::new(),
        };
        let hello = Hello { name: self.0.name.clone(), link_id: self.0.link_id.clone(), token, ..Hello::default() };
        let ok = match hello::client_handshake(&mut car.reader, &mut *car.writer, &hello) {
            Ok(ok) => ok,
            Err(e) => {
                (car.shutdown)();
                return Err(match e {
                    hello::HandshakeError::Refused(m) | hello::HandshakeError::Mismatch(m) => Error::Refused(CLOSE_NORMAL, m),
                    hello::HandshakeError::Io(e) => Error::LinkLost(e.to_string()),
                });
            }
        };
        *self.0.caps.lock().unwrap() = ok.caps.clone();
        let conn = Conn::start(Io { reader: car.reader, writer: car.writer, shutdown: car.shutdown }, ok, true, None, Hooks::default());
        if let Some(s) = settle {
            s();
        }
        let mut ever = self.0.ever.lock().unwrap();
        if *ever {
            let seqs: Vec<(String, u64)> = self.0.seqs.lock().unwrap().iter().map(|(k, v)| (k.clone(), *v)).collect();
            conn.send_control(hello::encode_resume(&Resume { link_id: self.0.link_id.clone(), seqs }));
        }
        *ever = true;
        *slot = Some(conn.clone());
        Ok(conn)
    }

    /// The services the endpoint listed in HELLO_OK (`name/version`).
    pub fn caps(&self) -> Vec<String> {
        self.0.caps.lock().unwrap().clone()
    }

    /// A client of `target` (`<host>:<app|pid>`) through the hub `hub`: the same requests,
    /// grabs included, as on a local endpoint.
    pub fn routed(hub: &Client, target: &str) -> Client {
        Client::new(Target::Routed(hub.clone(), target.to_string()), "client")
    }

    /// Opens a channel against `service`.
    pub fn open(&self, service: &str, args: &str) -> Result<Channel, Error> {
        if let Target::Routed(hub, target) = &self.0.target {
            return hub.open("route", &format!("{target} {service} {args}"));
        }
        let conn = self.conn()?;
        match conn.open(service, args) {
            Ok(ch) => Ok(ch),
            Err((CLOSE_LINK_LOST, why)) => Err(Error::LinkLost(why)),
            Err((code, why)) => Err(Error::Refused(code, why)),
        }
    }

    /// Bounds how long a `cmd` answer (and each message of a `frames` stream, see
    /// [`Client::timeout`]) may take from now on: past it the request's channel is closed and
    /// the reader gets [`Error::Timeout`]. `None`: wait as long as the app takes.
    pub fn set_timeout(&self, timeout: Option<Duration>) {
        *self.0.timeout.lock().unwrap() = timeout;
    }

    pub fn timeout(&self) -> Option<Duration> {
        *self.0.timeout.lock().unwrap()
    }

    /// Sends one request line and returns the answer's lines (the `end` line excluded; read it
    /// with [`Lines::end`] after the iterator ends).
    pub fn cmd(&self, line: &str) -> Result<Lines, Error> {
        let ch = self.open("cmd", "")?;
        ch.send(line.as_bytes().to_vec()).map_err(|e| Error::LinkLost(e.to_string()))?;
        Ok(Lines { ch: Some(ch), pending: VecDeque::new(), end: None, deadline: self.timeout().map(|t| std::time::Instant::now() + t) })
    }

    /// Follows the `watch` service (events) or `stream` service (chunks).
    pub fn feed(&self, service: &str) -> Feed {
        Feed { client: self.clone(), service: service.to_string(), ch: None }
    }

    pub fn watch(&self) -> Feed {
        self.feed("watch")
    }

    pub fn stream(&self) -> Feed {
        self.feed("stream")
    }

    /// Ends the connection (the endpoint keeps running).
    pub fn close(&self) {
        if let Some(c) = self.0.conn.lock().unwrap().take() {
            c.close("client done");
        }
    }
}

/// The lines of one `cmd` answer.
pub struct Lines {
    ch: Option<Channel>,
    pending: VecDeque<String>,
    end: Option<String>,
    deadline: Option<std::time::Instant>,
}

impl Lines {
    /// The `{"end":...}` line, once the iterator has finished (it carries `truncated`).
    pub fn end(&self) -> Option<&str> {
        self.end.as_deref()
    }
}

impl Iterator for Lines {
    type Item = Result<String, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(l) = self.pending.pop_front() {
                if l.starts_with("{\"end\":") {
                    self.end = Some(l);
                    self.pending.clear();
                    self.ch = None;
                    return None;
                }
                return Some(Ok(l));
            }
            let ch = self.ch.as_ref()?;
            let event = match self.deadline {
                None => ch.recv(),
                Some(deadline) => match ch.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
                    Ok(e) => Some(e),
                    Err(true) => {
                        // dropping the channel closes it; the connection goes on
                        self.ch = None;
                        return Some(Err(Error::Timeout));
                    }
                    Err(false) => None,
                },
            };
            match event {
                Some(Event::Data(d)) => self.pending.extend(String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()).map(str::to_string)),
                Some(Event::Closed(code, why)) => {
                    self.ch = None;
                    return match code {
                        CLOSE_LINK_LOST => Some(Err(Error::LinkLost(why))),
                        CLOSE_NORMAL => None,
                        _ => Some(Err(Error::Refused(code, why))),
                    };
                }
                Some(Event::Opened(_)) => {}
                None => {
                    self.ch = None;
                    return Some(Err(Error::LinkLost("channel closed".into())));
                }
            }
        }
    }
}

/// A `watch` or `stream` subscription that survives link loss: on a drop it reconnects, RESUME
/// continues after the last record seen and a gap shows up as the service's DROP record.
pub struct Feed {
    client: Client,
    service: String,
    ch: Option<Channel>,
}

impl Feed {
    /// The next `(seq, record)`; `Ok(None)` after `wait` with nothing new. Reconnects (with
    /// backoff) when the link is lost, and fails with the error of the last try after
    /// `retries` unsuccessful ones.
    pub fn next(&mut self, wait: Duration, retries: u32) -> Result<Option<(u64, Vec<u8>)>, Error> {
        let mut tries = 0u32;
        loop {
            if self.ch.is_none() {
                match self.client.open(&self.service, "") {
                    Ok(ch) => self.ch = Some(ch),
                    Err(Error::LinkLost(w)) => {
                        tries += 1;
                        if tries > retries {
                            return Err(Error::LinkLost(w));
                        }
                        std::thread::sleep(Duration::from_millis(200 * u64::from(tries.min(10))));
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
            let Some(ch) = self.ch.as_ref() else { continue };
            match ch.recv_timeout(wait) {
                Ok(Event::Data(d)) if d.len() >= 8 => {
                    let seq = u64::from_le_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]);
                    self.client.0.seqs.lock().unwrap().insert(self.service.clone(), seq);
                    return Ok(Some((seq, d[8..].to_vec())));
                }
                Ok(Event::Closed(code, why)) => {
                    self.ch = None;
                    if code != CLOSE_LINK_LOST {
                        return Err(Error::Refused(code, why));
                    }
                }
                Ok(_) => {}
                Err(true) => return Ok(None),
                Err(false) => self.ch = None,
            }
        }
    }
}
