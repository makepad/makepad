use crate::{Body, Error, Message, MessageType, Result, Value, NO_REPLY_EXPECTED};
use std::{
    collections::{HashSet, VecDeque},
    ffi::OsString,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::{ffi::OsStringExt, net::UnixStream},
    },
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Address {
    Path(PathBuf),
    Abstract(Vec<u8>),
}

/// Parse one address entry. Connection::connect also accepts semicolon-separated
/// alternatives. Values are percent-decoded as bytes; `guid` is ignored.
pub fn parse_address(address: &str) -> Result<Address> {
    let params = address
        .strip_prefix("unix:")
        .ok_or_else(|| Error::invalid("only unix bus addresses are supported"))?;
    let mut result = None;
    for param in params.split(',') {
        let (key, value) = param
            .split_once('=')
            .ok_or_else(|| Error::invalid("malformed address parameter"))?;
        if key == "path" || key == "abstract" {
            if result.is_some() {
                return Err(Error::invalid("ambiguous unix address"));
            }
            let bytes = unescape(value)?;
            if bytes.is_empty() || bytes.contains(&0) {
                return Err(Error::invalid("empty or NUL-containing socket address"));
            }
            result = Some(if key == "path" {
                Address::Path(OsString::from_vec(bytes).into())
            } else {
                Address::Abstract(bytes)
            });
        }
    }
    result.ok_or_else(|| Error::invalid("unix address has no path or abstract name"))
}

fn unescape(value: &str) -> Result<Vec<u8>> {
    let mut bytes = value.bytes();
    let mut out = Vec::new();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let mut hex = || {
                bytes
                    .next()
                    .and_then(|b| (b as char).to_digit(16))
                    .ok_or_else(|| Error::invalid("bad address escape"))
            };
            let high = hex()?;
            let low = hex()?;
            out.push((high * 16 + low) as u8);
        } else {
            out.push(byte);
        }
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthReply {
    Ok(String),
    Rejected(Vec<String>),
    Data(String),
    Error(String),
}

/// Parse one complete SASL reply, including its CRLF terminator.
pub fn parse_auth_line(line: &[u8]) -> Result<AuthReply> {
    let text = std::str::from_utf8(line).map_err(|_| Error::invalid("non-ASCII auth reply"))?;
    let text = text
        .strip_suffix("\r\n")
        .ok_or_else(|| Error::invalid("auth reply needs CRLF"))?;
    if !text.is_ascii() || text.bytes().any(|b| b < 32 || b == 127) {
        return Err(Error::invalid("invalid auth reply"));
    }
    let (command, args) = text.split_once(' ').unwrap_or((text, ""));
    Ok(match command {
        "OK" if args.len() == 32 && args.bytes().all(|b| b.is_ascii_hexdigit()) => {
            AuthReply::Ok(args.into())
        }
        "REJECTED" => AuthReply::Rejected(args.split_whitespace().map(str::to_owned).collect()),
        "DATA" if args.len() % 2 == 0 && args.bytes().all(|b| b.is_ascii_hexdigit()) => {
            AuthReply::Data(args.into())
        }
        "ERROR" => AuthReply::Error(args.into()),
        _ => return Err(Error::invalid("malformed or unexpected auth command")),
    })
}

extern "C" {
    fn getuid() -> u32;
}

fn external_identity() -> String {
    // getuid has no pointer arguments and returns the real Unix uid.
    unsafe { getuid() }
        .to_string()
        .bytes()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn authenticate(stream: &mut UnixStream, deadline: Instant) -> Result<()> {
    let identity = external_identity();
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    stream.write_all(format!("\0AUTH EXTERNAL {identity}\r\n").as_bytes())?;
    loop {
        let mut line = Vec::new();
        while !line.ends_with(b"\r\n") {
            stream.set_read_timeout(Some(remaining(deadline)?))?;
            let mut byte = [0];
            stream.read_exact(&mut byte)?;
            line.push(byte[0]);
        }
        match parse_auth_line(&line)? {
            AuthReply::Ok(_) => {
                stream.set_write_timeout(Some(remaining(deadline)?))?;
                stream.write_all(b"BEGIN\r\n")?;
                return Ok(());
            }
            AuthReply::Data(challenge) if challenge.is_empty() => {
                stream.set_write_timeout(Some(remaining(deadline)?))?;
                stream.write_all(format!("DATA {identity}\r\n").as_bytes())?;
            }
            other => {
                return Err(Error::invalid(format!(
                    "EXTERNAL authentication refused: {other:?}"
                )))
            }
        }
    }
}

struct Outgoing {
    bytes: Vec<u8>,
    written: usize,
}

/// Single-owner connection for a caller's worker/poll loop. Setup and `call`
/// block; `send`, `read_ready`, and `flush` never wait for readiness. Unrelated
/// messages remain queued during `call`. No threads or locks are created.
pub struct Connection {
    stream: UnixStream,
    serial: u32,
    pending: HashSet<u32>,
    input: Vec<u8>,
    output: VecDeque<Outgoing>,
    messages: VecDeque<Message>,
    closed: bool,
    unique_name: Option<String>,
}

impl Connection {
    pub fn session(timeout: Duration) -> Result<Self> {
        if let Ok(address) = std::env::var("DBUS_SESSION_BUS_ADDRESS") {
            if !address.is_empty() {
                return Self::connect(&address, timeout);
            }
        }
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| Error::invalid("DBUS_SESSION_BUS_ADDRESS is not set"))?;
        let deadline = deadline(timeout)?;
        Self::connect_stream(
            UnixStream::connect(PathBuf::from(dir).join("bus"))?,
            deadline,
        )
    }
    /// Authenticate and Hello on a bus (including an accessibility bus address).
    pub fn connect(address: &str, timeout: Duration) -> Result<Self> {
        let deadline = deadline(timeout)?;
        let mut last = Error::invalid("no usable bus address");
        for entry in address.split(';') {
            remaining(deadline)?;
            let result = (|| {
                let stream = match parse_address(entry)? {
                    Address::Path(path) => UnixStream::connect(path)?,
                    Address::Abstract(name) => {
                        #[cfg(target_os = "linux")]
                        {
                            use std::os::linux::net::SocketAddrExt;
                            let addr = std::os::unix::net::SocketAddr::from_abstract_name(name)?;
                            UnixStream::connect_addr(&addr)?
                        }
                        #[cfg(not(target_os = "linux"))]
                        {
                            let _ = name;
                            return Err(Error::invalid("abstract sockets require Linux"));
                        }
                    }
                };
                Self::connect_stream(stream, deadline)
            })();
            match result {
                Ok(connection) => return Ok(connection),
                Err(error) => last = error,
            }
        }
        Err(last)
    }
    fn connect_stream(mut stream: UnixStream, deadline: Instant) -> Result<Self> {
        authenticate(&mut stream, deadline)?;
        let mut connection = Self::from_authenticated_stream(stream)?;
        connection.hello(remaining(deadline)?)?;
        Ok(connection)
    }
    /// Adopt a stream after the caller has completed authentication/BEGIN.
    /// Does not send Hello; useful for peer connections and custom transports.
    pub fn from_authenticated_stream(stream: UnixStream) -> Result<Self> {
        stream.set_read_timeout(None)?;
        stream.set_write_timeout(None)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            serial: 0,
            pending: HashSet::new(),
            input: Vec::new(),
            output: VecDeque::new(),
            messages: VecDeque::new(),
            closed: false,
            unique_name: None,
        })
    }
    pub fn fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }
    pub fn unique_name(&self) -> Option<&str> {
        self.unique_name.as_deref()
    }
    pub fn wants_write(&self) -> bool {
        !self.output.is_empty()
    }
    pub fn is_closed(&self) -> bool {
        self.closed
    }
    /// Assign a fresh serial and queue a message. A successful send is retained
    /// until flushed, including across WouldBlock and partial writes.
    pub fn send(&mut self, mut message: Message) -> Result<u32> {
        if self.closed {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "bus closed").into());
        }
        if message.header.unix_fds != 0 {
            return Err(Error::invalid("descriptor passing is not negotiated"));
        }
        loop {
            self.serial = self.serial.wrapping_add(1).max(1);
            if !self.pending.contains(&self.serial) {
                break;
            }
        }
        message.header.serial = self.serial;
        let bytes = message.encode()?;
        if message.header.kind == MessageType::MethodCall
            && message.header.flags & NO_REPLY_EXPECTED == 0
        {
            self.pending.insert(self.serial);
        }
        self.output.push_back(Outgoing { bytes, written: 0 });
        Ok(self.serial)
    }
    pub fn emit_signal(
        &mut self,
        path: &str,
        interface: &str,
        member: &str,
        body: Body,
    ) -> Result<u32> {
        self.send(Message::signal(path, interface, member, body))
    }
    /// Write queued bytes until the socket would block. Check wants_write after.
    pub fn flush(&mut self) -> Result<()> {
        self.flush_until(None)
    }
    fn flush_until(&mut self, deadline: Option<Instant>) -> Result<()> {
        while let Some(front) = self.output.front_mut() {
            if let Some(deadline) = deadline {
                remaining(deadline)?;
            }
            match self.stream.write(&front.bytes[front.written..]) {
                Ok(0) => {
                    return Err(
                        io::Error::new(io::ErrorKind::WriteZero, "bus write returned zero").into(),
                    )
                }
                Ok(n) => {
                    front.written += n;
                    if front.written == front.bytes.len() {
                        self.output.pop_front();
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    /// Read and frame available messages, returning how many were enqueued.
    /// EOF after complete messages is reported via is_closed; drain them first.
    /// A protocol/I/O error makes the connection unusable; queued messages can
    /// still be drained. The caller must reconnect rather than retry bad bytes.
    pub fn read_ready(&mut self) -> Result<usize> {
        self.read_until(None)
    }
    fn read_until(&mut self, deadline: Option<Instant>) -> Result<usize> {
        let result = self.read_available(deadline);
        if result.as_ref().is_err_and(|e| !matches!(e, Error::Timeout)) {
            self.closed = true;
        }
        result
    }
    fn read_available(&mut self, deadline: Option<Instant>) -> Result<usize> {
        let mut count = 0;
        let mut chunk = [0; 16384];
        loop {
            if let Some(deadline) = deadline {
                if count != 0 && remaining(deadline).is_err() {
                    return Ok(count);
                }
                remaining(deadline)?;
            }
            if self.closed {
                return if count != 0 {
                    Ok(count)
                } else {
                    Err(io::Error::new(io::ErrorKind::UnexpectedEof, "bus closed").into())
                };
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.closed = true;
                    if !self.input.is_empty() {
                        return Err(Error::invalid("bus closed during a message"));
                    }
                }
                Ok(n) => {
                    self.input.extend_from_slice(&chunk[..n]);
                    let mut consumed = 0;
                    while let Some(len) = Message::frame_len(&self.input[consumed..])? {
                        if self.input.len() - consumed < len {
                            break;
                        }
                        // Future message types are ignored as required by the
                        // wire protocol; type zero remains invalid.
                        if self.input[consumed + 1] > 4 {
                            consumed += len;
                            continue;
                        }
                        let message = Message::decode(&self.input[consumed..consumed + len])?;
                        if message.header.unix_fds != 0 {
                            return Err(Error::invalid("received unnegotiated descriptors"));
                        }
                        self.messages.push_back(message);
                        consumed += len;
                        count += 1;
                    }
                    if consumed != 0 {
                        self.input.drain(..consumed);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(count),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }
    pub fn pop_message(&mut self) -> Option<Message> {
        let message = self.messages.pop_front()?;
        self.release_reply(&message);
        Some(message)
    }
    fn release_reply(&mut self, message: &Message) {
        if matches!(
            message.header.kind,
            MessageType::MethodReturn | MessageType::Error
        ) {
            if let Some(serial) = message.header.reply_serial {
                self.pending.remove(&serial);
            }
        }
    }
    pub fn take_reply(&mut self, serial: u32) -> Option<Message> {
        let at = self.messages.iter().position(|m| {
            m.header.reply_serial == Some(serial)
                && matches!(
                    m.header.kind,
                    MessageType::MethodReturn | MessageType::Error
                )
        })?;
        let message = self.messages.remove(at)?;
        self.release_reply(&message);
        Some(message)
    }
    /// Stop reserving a serial after the caller abandons an asynchronous call.
    /// Does not retract queued bytes or discard a late reply.
    pub fn forget_reply(&mut self, serial: u32) {
        self.pending.remove(&serial);
    }
    /// Blocking setup helper. Incoming calls/signals are queued, not dispatched.
    /// Timeout does not cancel a remote operation or retract queued bytes.
    pub fn call(&mut self, message: Message, timeout: Duration) -> Result<Message> {
        if message.header.kind != MessageType::MethodCall
            || message.header.flags & NO_REPLY_EXPECTED != 0
        {
            return Err(Error::invalid("call requires a method expecting a reply"));
        }
        let deadline = deadline(timeout)?;
        remaining(deadline)?;
        let serial = self.send(message)?;
        let result = (|| loop {
            if let Some(reply) = self.take_reply(serial) {
                return reply.into_result();
            }
            remaining(deadline)?;
            self.flush_until(Some(deadline))?;
            self.read_until(Some(deadline))?;
            if let Some(reply) = self.take_reply(serial) {
                return reply.into_result();
            }
            self.wait(deadline)?;
        })();
        self.pending.remove(&serial);
        result
    }
    fn bus_call(&mut self, member: &str, body: Body, timeout: Duration) -> Result<Message> {
        self.call(
            Message::method_call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                member,
                body,
            ),
            timeout,
        )
    }
    pub fn hello(&mut self, timeout: Duration) -> Result<&str> {
        if self.unique_name.is_none() {
            let reply = self.bus_call("Hello", Body::default(), timeout)?;
            if reply.body.signature() != "s" {
                return Err(Error::invalid("invalid Hello reply"));
            }
            let name = reply.body.reader().str()?.to_owned();
            crate::signature::validate_bus_name(&name)?;
            if !name.starts_with(':') {
                return Err(Error::invalid("Hello did not return a unique name"));
            }
            self.unique_name = Some(name);
        }
        Ok(self.unique_name.as_deref().unwrap())
    }
    /// Return the bus's RequestName status code (1 owner, 2 queued, 3 exists, 4 already owner).
    pub fn request_name(&mut self, name: &str, flags: u32, timeout: Duration) -> Result<u32> {
        crate::signature::validate_bus_name(name)?;
        let reply = self.bus_call(
            "RequestName",
            Body::from_values(&[Value::Str(name.into()), Value::U32(flags)])?,
            timeout,
        )?;
        if reply.body.signature() != "u" {
            return Err(Error::invalid("invalid RequestName reply"));
        }
        reply.body.reader().u32()
    }
    pub fn add_match(&mut self, rule: &str, timeout: Duration) -> Result<()> {
        self.match_rule("AddMatch", rule, timeout)
    }
    pub fn remove_match(&mut self, rule: &str, timeout: Duration) -> Result<()> {
        self.match_rule("RemoveMatch", rule, timeout)
    }
    fn match_rule(&mut self, member: &str, rule: &str, timeout: Duration) -> Result<()> {
        let reply = self.bus_call(
            member,
            Body::from_values(&[Value::Str(rule.into())])?,
            timeout,
        )?;
        if !reply.body.signature().is_empty() {
            return Err(Error::invalid("invalid match-rule reply"));
        }
        Ok(())
    }
    fn wait(&self, deadline: Instant) -> Result<()> {
        loop {
            let time = remaining(deadline)?;
            let ms = time
                .as_millis()
                .saturating_add(u128::from(time.subsec_nanos() % 1_000_000 != 0))
                .min(i32::MAX as u128) as i32;
            let mut fd = PollFd {
                fd: self.fd(),
                events: 1 | if self.wants_write() { 4 } else { 0 },
                revents: 0,
            };
            // One valid pollfd, held for the duration of the synchronous call.
            let result = unsafe { poll(&mut fd, 1, ms) };
            if result > 0 {
                return Ok(());
            }
            if result < 0 {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted {
                    return Err(e.into());
                }
            }
        }
    }
}

fn deadline(timeout: Duration) -> Result<Instant> {
    Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| Error::invalid("timeout out of range"))
}
fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or(Error::Timeout)
}
#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}
#[cfg(any(target_os = "linux", target_os = "android"))]
type Nfds = std::os::raw::c_ulong;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
type Nfds = u32;
extern "C" {
    fn poll(fds: *mut PollFd, nfds: Nfds, timeout: i32) -> i32;
}
