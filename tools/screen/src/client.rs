//! An attach client only transports input and the daemon's safe VT projection.
//! A startup probe captures the display theme; only the host answers child queries.
use crate::protocol;
#[cfg(unix)]
use crate::{
    protocol::SessionLocation,
    unix::{self, NonblockingGuard, PollFd, RawTerminal, SignalGuard},
};
use crate::{server::StartOptions, theme::DisplayColors};
#[cfg(unix)]
use makepad_strict_json::{self as json, Value};
use std::{
    collections::VecDeque,
    io,
    time::{Duration, Instant},
};
#[cfg(unix)]
use std::{
    io::{Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
};

#[cfg(windows)]
#[path = "windows_console.rs"]
pub(crate) mod windows_console;
#[cfg(windows)]
#[path = "client_windows.rs"]
mod windows_impl;
#[cfg(windows)]
pub use windows_impl::{attach, start_attached};

const OUTPUT_LIMIT: usize = 4 * protocol::MAX_FRAME;
const INPUT_LIMIT: usize = 256 * 1024;
const INPUT_CHUNK: usize = 4096;
const RESTORE_VT: &[u8] = concat!(
    "\x18\x1b\\\x1b[?2026l\x1b[=0;1u\x1b[?1049l\x1b[?47l\x1b[0m\x1b[2l\x1b[20l\x1b[?25h\x1b[?1l\x1b>\x1b[?5l\x1b[?67l\x1b[?9l",
    "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?1016l",
    "\x1b[?1007h\x1b[?1035h\x1b[?1036h\x1b[?1039l\x1b[?2004l\x1b[?2027l\x1b[?2031l\x1b[?2048l",
    "\x1b[=0;1u\x1b[>4;0m\x1b[0 q"
).as_bytes();

#[cfg(unix)]
pub fn attach(location: &SessionLocation, read_only: bool) -> Result<(), String> {
    attach_inner(location, read_only, None)
}

#[cfg(unix)]
pub fn start_attached(options: StartOptions, restart: bool) -> Result<(), String> {
    let location = SessionLocation::open(&options.state_dir, &options.session_id, true)?;
    attach_inner(&location, false, Some((options, restart)))
}

#[cfg(unix)]
fn attach_inner(
    location: &SessionLocation,
    read_only: bool,
    start: Option<(StartOptions, bool)>,
) -> Result<(), String> {
    let signals =
        SignalGuard::new().map_err(|error| format!("Install attach signal handling: {error}"))?;
    let mut raw = RawTerminal::new(0).map_err(|error| format!("Set terminal raw mode: {error}"))?;
    let mut input_flags = NonblockingGuard::new(0)
        .map_err(|error| format!("Set nonblocking terminal input: {error}"))?;
    let mut output_flags = NonblockingGuard::new(1)
        .map_err(|error| format!("Set nonblocking terminal output: {error}"))?;
    let mut colors = DisplayColors::default();
    let result = (|| {
        if unix::is_tty(0) && unix::is_tty(1) {
            colors.capture(
                || {
                    let mut buffer = [0u8; 4096];
                    match unix::read_fd(0, &mut buffer) {
                        Ok(count) => Ok(Some(buffer[..count].to_vec())),
                        Err(e)
                            if matches!(
                                e.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                            ) =>
                        {
                            Ok(None)
                        }
                        Err(e) => Err(e),
                    }
                },
                |bytes| unix::write_fd(1, bytes),
                || signals.interrupted().is_some(),
            )?;
        }
        if let Some((mut options, restart)) = start {
            options.theme.merge_missing(&colors.theme);
            if unix::is_tty(1) {
                (options.cols, options.rows) = client_size(Some(1))?;
            }
            crate::server::start(options, restart)?;
        }
        if !location.validate_socket()? {
            return Err(
                "Screen session has no live endpoint; attach never starts a replacement".into(),
            );
        }
        let stream = unix::connect(&location.socket_path, Duration::from_secs(3))
            .map_err(|error| format!("Connect screen: {error}"))?;
        if unix::peer_uid(&stream).map_err(|error| format!("Read screen peer identity: {error}"))?
            != unix::current_uid()
        {
            return Err("Screen socket belongs to another user".into());
        }
        stream
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        attach_loop(stream, location, read_only, &signals, &mut colors)
    })();
    // Restore both emulator presentation and kernel termios before the CLI
    // prints status/errors. Every early return also retains RAII restoration.
    let presentation = if unix::is_tty(1) {
        let mut restore = RESTORE_VT.to_vec();
        restore.extend(colors.restore());
        restore_vt(&restore)
    } else {
        Ok(())
    };
    let raw_result = raw.restore();
    let input_result = input_flags.restore();
    let output_result = output_flags.restore();
    for (name, restored) in [
        ("terminal presentation", presentation),
        ("terminal mode", raw_result),
        ("stdin flags", input_result),
        ("stdout flags", output_result),
    ] {
        if let Err(error) = restored {
            return Err(format!(
                "Could not restore {name}: {error}; attach outcome: {}",
                result
                    .as_ref()
                    .err()
                    .map(String::as_str)
                    .unwrap_or("detached")
            ));
        }
    }
    match result {
        Ok(reason) => {
            eprintln!("makepad-screen: {} · {}", location.session_id, reason);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn attach_loop(
    mut stream: UnixStream,
    location: &SessionLocation,
    read_only: bool,
    signals: &SignalGuard,
    colors: &mut DisplayColors,
) -> Result<&'static str, String> {
    let size_fd = if unix::is_tty(1) {
        Some(1)
    } else if unix::is_tty(0) {
        Some(0)
    } else {
        None
    };
    let mut size = client_size(size_fd)?;
    let hello = json::obj(vec![
        ("version", Value::Int(protocol::VERSION as i64)),
        ("session_id", json::s(&location.session_id)),
        ("read_only", Value::Bool(read_only)),
        ("cols", Value::Int(size.0 as i64)),
        ("rows", Value::Int(size.1 as i64)),
    ])
    .to_json();
    let mut outgoing = Queue::new(INPUT_LIMIT);
    outgoing.push(protocol::encode_frame(protocol::HELLO, hello.as_bytes())?)?;
    let mut output = Queue::new(OUTPUT_LIMIT);
    let mut input = InputDecoder::default();
    let (initial_input, detached) = input.push(&std::mem::take(&mut colors.initial_input));
    if detached {
        return Ok("detached; session kept running");
    }
    if !read_only && !initial_input.is_empty() {
        outgoing.push(protocol::encode_frame(protocol::INPUT, &initial_input)?)?;
    }
    let mut received = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut ready = false;
    let started = Instant::now();
    let mut last_size = started;
    let mut closing: Option<(Result<&'static str, String>, Instant)> = None;
    loop {
        if let Some(signal) = signals.interrupted() {
            return Err(format!(
                "Attach interrupted by signal {signal}; session was not stopped"
            ));
        }
        if closing.is_none() && !ready && started.elapsed() > Duration::from_secs(5) {
            return Err("Screen did not send an initial snapshot within five seconds".into());
        }
        if closing.is_none() && last_size.elapsed() >= Duration::from_millis(100) {
            last_size = Instant::now();
            let next = client_size(size_fd)?;
            if next != size && outgoing.remaining() >= 256 {
                let dimensions = json::obj(vec![
                    ("cols", Value::Int(next.0 as i64)),
                    ("rows", Value::Int(next.1 as i64)),
                ])
                .to_json();
                outgoing.push(protocol::encode_frame(
                    protocol::RESIZE,
                    dimensions.as_bytes(),
                )?)?;
                size = next;
            }
        }
        if closing.is_none() && outgoing.remaining() >= 1024 {
            let (bytes, detach) = input.push(&colors.expired_input());
            if !read_only && !bytes.is_empty() {
                outgoing.push(protocol::encode_frame(protocol::INPUT, &bytes)?)?;
            }
            if detach {
                return Ok("detached; session kept running");
            }
            if let Some(bytes) = input.expired_sequence() {
                if !read_only {
                    outgoing.push(protocol::encode_frame(protocol::INPUT, &bytes)?)?;
                }
            }
        }
        // Decode only when the complete next output frame can fit. A slow
        // outer terminal backpressures the socket; no frame is discarded.
        while closing.is_none() && output.remaining() >= protocol::MAX_FRAME {
            let Some(frame) = protocol::decode_frame(&mut received)? else {
                break;
            };
            match frame.kind {
                protocol::SNAPSHOT | protocol::OUTPUT => {
                    ready = true;
                    colors.observe_output(&frame.payload);
                    output.push(frame.payload)?;
                }
                protocol::EXIT => {
                    let value = protocol::parse_object(&frame.payload)?;
                    let reason = value
                        .get("reason")
                        .and_then(Value::as_str)
                        .map(clean_message)
                        .unwrap_or_else(|| "session ended".into());
                    let result = match value.get("code") {
                        Some(Value::Int(0)) => Ok("session exited (0)"),
                        Some(Value::Int(code)) => {
                            Err(format!("Session exited with status {code}: {reason}"))
                        }
                        _ => Err(format!(
                            "Session ended without an observed exit status: {reason}"
                        )),
                    };
                    outgoing.clear();
                    closing = Some((result, Instant::now()));
                }
                protocol::ERROR => {
                    outgoing.clear();
                    closing = Some((
                        Err(format!(
                            "Screen: {}",
                            clean_message(&String::from_utf8_lossy(&frame.payload))
                        )),
                        Instant::now(),
                    ));
                }
                kind => return Err(format!("Unexpected frame {kind} on screen attachment")),
            }
        }
        if let Some((_, since)) = &closing {
            if output.is_empty() && outgoing.is_empty() {
                return closing.take().unwrap().0;
            }
            if since.elapsed() > Duration::from_secs(2) {
                return Err(
                    "Attach ended before buffered terminal data could drain within two seconds"
                        .into(),
                );
            }
        }
        let receive_socket = closing.is_none()
            && output.remaining() >= protocol::MAX_FRAME
            && received.len() < protocol::MAX_FRAME + 5;
        let read_input = closing.is_none() && outgoing.remaining() >= INPUT_CHUNK + 1024;
        let mut fds = [
            PollFd {
                fd: if read_input { 0 } else { -1 },
                events: unix::READABLE,
                revents: 0,
            },
            PollFd {
                fd: if receive_socket || !outgoing.is_empty() {
                    stream.as_raw_fd()
                } else {
                    -1
                },
                events: (if receive_socket { unix::READABLE } else { 0 })
                    | (if outgoing.is_empty() {
                        0
                    } else {
                        unix::WRITABLE
                    }),
                revents: 0,
            },
            PollFd {
                fd: if output.is_empty() { -1 } else { 1 },
                events: unix::WRITABLE,
                revents: 0,
            },
        ];
        match unix::poll(&mut fds, 100) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("Poll attachment: {error}")),
        }
        if fds[2].revents & (unix::WRITABLE | unix::POLL_ERROR) != 0 {
            output
                .flush(|bytes| unix::write_fd(1, bytes))
                .map_err(|error| format!("Write terminal output: {error}"))?;
        }
        if fds[1].revents & unix::WRITABLE != 0 {
            outgoing
                .flush(|bytes| stream.write(bytes))
                .map_err(|error| format!("Send screen input: {error}"))?;
        }
        if receive_socket && fds[1].revents & (unix::READABLE | unix::POLL_ERROR) != 0 {
            let room = (protocol::MAX_FRAME + 5)
                .saturating_sub(received.len())
                .min(buffer.len());
            match stream.read(&mut buffer[..room]) {
                Ok(0) => {
                    let bytes = input.flush_sequence();
                    if !read_only && !bytes.is_empty() {
                        outgoing.push(protocol::encode_frame(protocol::INPUT, &bytes)?)?;
                    }
                    outgoing.clear();
                    closing = Some((Err("Screen connection closed without an exit report; session state is unknown".into()), Instant::now()));
                }
                Ok(count) => received.extend_from_slice(&buffer[..count]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(format!("Read screen output: {error}")),
            }
        }
        if read_input && fds[0].revents & (unix::READABLE | unix::POLL_ERROR) != 0 {
            match unix::read_fd(0, &mut buffer[..INPUT_CHUNK]) {
                Ok(0) => {
                    closing = Some((Ok("detached (input closed)"), Instant::now()));
                }
                Ok(count) => {
                    let (bytes, detach) = input.push(&colors.filter_input(&buffer[..count]));
                    if !read_only && !bytes.is_empty() {
                        outgoing.push(protocol::encode_frame(protocol::INPUT, &bytes)?)?;
                    }
                    if detach {
                        closing = Some((Ok("detached; session kept running"), Instant::now()));
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(format!("Read terminal input: {error}")),
            }
        }
    }
}

#[cfg(unix)]
fn client_size(fd: Option<i32>) -> Result<(u16, u16), String> {
    let size = if let Some(fd) = fd {
        unix::window_size(fd).map_err(|error| format!("Read actual terminal size: {error}"))?
    } else {
        let dimension = |name: &str, default: u16| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(default)
        };
        (dimension("COLUMNS", 80), dimension("LINES", 24))
    };
    // Match the daemon's bounded terminal model without silently pretending a
    // larger real TTY has different dimensions.
    if !(2..=400).contains(&size.0) || !(2..=200).contains(&size.1) {
        return Err(format!(
            "Terminal size {}×{} is outside 2..400 columns by 2..200 rows; resize before attaching",
            size.0, size.1
        ));
    }
    Ok(size)
}

struct Queue {
    chunks: VecDeque<Vec<u8>>,
    offset: usize,
    bytes: usize,
    limit: usize,
}
impl Queue {
    fn new(limit: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            offset: 0,
            bytes: 0,
            limit,
        }
    }
    fn remaining(&self) -> usize {
        self.limit - self.bytes
    }
    fn is_empty(&self) -> bool {
        self.bytes == 0
    }
    fn clear(&mut self) {
        self.chunks.clear();
        self.offset = 0;
        self.bytes = 0;
    }
    fn push(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        if bytes.len() > self.remaining() {
            return Err("Attachment queue exceeded its bounded capacity".into());
        }
        if !bytes.is_empty() {
            self.bytes += bytes.len();
            self.chunks.push_back(bytes);
        }
        Ok(())
    }
    fn flush(&mut self, mut write: impl FnMut(&[u8]) -> io::Result<usize>) -> io::Result<()> {
        let mut budget = 256 * 1024;
        while budget > 0 {
            let Some(front) = self.chunks.front() else {
                break;
            };
            let bytes = &front[self.offset..(self.offset + budget).min(front.len())];
            match write(bytes) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "Attachment output closed",
                    ))
                }
                Ok(count) => {
                    self.offset += count;
                    self.bytes -= count;
                    budget -= count;
                    if self.offset == front.len() {
                        self.chunks.pop_front();
                        self.offset = 0;
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    break
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct InputDecoder {
    paste: bool,
    marker: usize,
    sequence: Vec<u8>,
    sequence_since: Option<Instant>,
}
impl InputDecoder {
    fn push(&mut self, bytes: &[u8]) -> (Vec<u8>, bool) {
        let mut output = Vec::with_capacity(bytes.len() + self.sequence.len());
        for &byte in bytes {
            if self.paste {
                let marker = b"\x1b[201~";
                if byte == marker[self.marker] {
                    self.marker += 1;
                    if self.marker == marker.len() {
                        self.paste = false;
                        self.marker = 0;
                    }
                } else {
                    self.marker = usize::from(byte == 0x1b);
                }
                output.push(byte);
                continue;
            }
            if !self.sequence.is_empty() {
                self.sequence.push(byte);
                if self.sequence.len() == 2 && byte == b'[' {
                    continue;
                }
                if self.sequence.len() > 2
                    && (0x20..=0x3f).contains(&byte)
                    && self.sequence.len() < 64
                {
                    continue;
                }
                let sequence = self.flush_sequence();
                if ctrl_d_sequence(&sequence) {
                    return (output, true);
                }
                if sequence == b"\x1b[200~" {
                    self.paste = true;
                }
                output.extend(sequence);
            } else if byte == 0x04 {
                // A plain keyboard Ctrl+D detaches only this view.
                return (output, true);
            } else if byte == 0x1b {
                self.sequence.push(byte);
                self.sequence_since = Some(Instant::now());
            } else {
                output.push(byte);
            }
        }
        (output, false)
    }
    fn flush_sequence(&mut self) -> Vec<u8> {
        self.sequence_since = None;
        std::mem::take(&mut self.sequence)
    }
    fn expired_sequence(&mut self) -> Option<Vec<u8>> {
        // Keep a standalone Escape responsive while accepting fragmented
        // Kitty/CSI-u and xterm key reports from the outer terminal.
        self.sequence_since
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(25))
            .then(|| self.flush_sequence())
    }
}

fn ctrl_d_sequence(bytes: &[u8]) -> bool {
    let Some(body) = bytes.strip_prefix(b"\x1b[") else {
        return false;
    };
    let control_only = |modifiers: &str| {
        modifiers
            .parse::<u32>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .is_some_and(|bits| bits & !(64 | 128) == 4)
    };
    if let Some(body) = body
        .strip_suffix(b"u")
        .and_then(|b| std::str::from_utf8(b).ok())
    {
        let mut fields = body.split(';');
        let key = fields.next().unwrap_or("").split(':').next().unwrap_or("");
        let mut modifiers = fields.next().unwrap_or("").split(':');
        let control = control_only(modifiers.next().unwrap_or(""));
        let pressed = matches!(modifiers.next(), None | Some("1") | Some("2"));
        return key == "100" && control && pressed && modifiers.next().is_none();
    }
    if let Some(body) = body
        .strip_suffix(b"~")
        .and_then(|b| std::str::from_utf8(b).ok())
    {
        let mut fields = body.split(';');
        return fields.next() == Some("27")
            && control_only(fields.next().unwrap_or(""))
            && fields.next() == Some("100")
            && fields.next().is_none();
    }
    false
}

fn clean_message(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(400)
        .collect()
}
#[cfg(unix)]
fn restore_vt(bytes: &[u8]) -> io::Result<()> {
    let mut offset = 0;
    let deadline = Instant::now() + Duration::from_millis(250);
    while offset < bytes.len() {
        match unix::write_fd(1, &bytes[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Terminal output closed",
                ))
            }
            Ok(count) => offset += count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Terminal output did not drain",
                    ));
                }
                let mut fds = [PollFd {
                    fd: 1,
                    events: unix::WRITABLE,
                    revents: 0,
                }];
                match unix::poll(&mut fds, 20) {
                    Err(error) if error.kind() != io::ErrorKind::Interrupted => return Err(error),
                    _ => {}
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
