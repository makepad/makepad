//! A serial line on another machine: [`RemoteLine`], the [`Line`] a device protocol runs over when
//! its port is on another PC, and [`serve`], the node end that runs the line's ops on the port.
//!
//! The stream between them (a local socket a bridge carries to the node) speaks wire v1. Each side
//! first sends `MPSERIAL` and its version (`u16`, little-endian); then frames go both ways:
//! `len: u32` (the bytes after this field), `kind: u8`, `seq: u32`, payload, all integers
//! little-endian. The client numbers its ops from 1; an answer carries the number of the op it
//! answers. One op is outstanding at a time, so the node runs them in order: a reset sequence
//! that started runs to its end even when the client goes, and no board is left held in reset.
//!
//! Every timeout runs on the node, where the port is: a `want` asks for bytes within its timeout
//! and gets exactly one answer, the bytes or `quiet`. A write has no answer; its failure answers
//! the next op that waits. A stream that ends or goes silent past an op's own time is
//! `ConnectionAborted`, never `TimedOut`: the board did not time out, the link to its PC dropped.

use crate::line::{Line, Step, CONNECT_WINDOW};
use std::collections::VecDeque;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::time::Duration;

const MAGIC: &[u8; 8] = b"MPSERIAL";
/// The wire version this build speaks.
pub const VERSION: u16 = 1;

/// Client ops.
const OPEN: u8 = 1;
const WRITE: u8 = 2;
const WANT: u8 = 3;
const DRAIN: u8 = 4;
const SEQUENCE: u8 = 5;
const BAUD: u8 = 6;
const REOPEN: u8 = 7;
const PING: u8 = 8;
const CLOSE: u8 = 9;

/// Node answers.
const OK: u8 = 64;
const ERR: u8 = 65;
const DATA: u8 = 66;
const QUIET: u8 = 67;
const DRAINED: u8 = 68;
const FAILED: u8 = 69;
const PONG: u8 = 70;
/// Bytes the node read with no `want` outstanding (v2's monitor); v1's node sends none.
const PUSH: u8 = 71;

/// Sequence step tags.
const STEP_DTR: u8 = 1;
const STEP_RTS: u8 = 2;
const STEP_WAIT: u8 = 3;
const STEP_DRAIN: u8 = 4;

/// The most bytes one `data` answer carries; the rest wait in the port for the next `want`.
const WANT_BYTES: usize = 16 * 1024;
/// The most bytes one `write` frame carries; a longer write goes as several, in order.
const WRITE_BYTES: usize = 64 * 1024;
/// A frame longer than this is no frame of this wire (each side writes far shorter ones): the
/// stream is corrupt, and the session ends rather than allocate what its length says.
const FRAME_LIMIT: usize = 1 << 24;
/// Lead choice: how long past an op's own time the client waits for its answer before it calls
/// the link to the board's PC dead. A bridge that drops closes the stream at once; this bounds a
/// link that stalls without closing.
pub const LINK_SLACK: Duration = Duration::from_secs(30);

/// What the session opens the device as. v1 has the serial port only; a later debug or JTAG mode
/// is another variant on the same wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Serial,
}

impl Mode {
    fn code(self) -> u8 {
        match self { Mode::Serial => 0 }
    }

    fn of(code: u8) -> io::Result<Mode> {
        match code {
            0 => Ok(Mode::Serial),
            _ => Err(io::Error::new(io::ErrorKind::Unsupported, format!("open mode {code} is not on serial wire v{VERSION}"))),
        }
    }
}

/// Opens the session's device on the node: the bridge's, which checks the device's identity and
/// its serial.conf verdict before every open.
pub trait Opener {
    fn open(&mut self, mode: Mode, baud: u32) -> io::Result<Box<dyn Line + Send>>;
    /// After a USB re-enumeration: resolve the device by its identity again (serial.conf and
    /// identity re-checked) and open it at the session's current baud within `window`.
    fn reopen(&mut self, mode: Mode, baud: u32, window: Duration) -> io::Result<Box<dyn Line + Send>>;
}

/// The wire's error kinds, from the io kinds a line or an opener reports, and back.
fn err_code(kind: io::ErrorKind) -> u8 {
    match kind {
        io::ErrorKind::TimedOut => 1,
        io::ErrorKind::NotFound => 2,
        io::ErrorKind::PermissionDenied => 3,
        io::ErrorKind::ResourceBusy => 4,
        _ => 5,
    }
}

fn err_kind(code: u8) -> io::ErrorKind {
    match code {
        1 => io::ErrorKind::TimedOut,
        2 => io::ErrorKind::NotFound,
        3 => io::ErrorKind::PermissionDenied,
        4 => io::ErrorKind::ResourceBusy,
        _ => io::ErrorKind::Other,
    }
}

fn err_payload(err: &io::Error) -> Vec<u8> {
    let mut payload = vec![err_code(err.kind())];
    payload.extend_from_slice(err.to_string().as_bytes());
    payload
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("serial wire v{VERSION}: {what}"))
}

fn write_frame(w: &mut impl Write, kind: u8, seq: u32, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(5 + payload.len()).map_err(|_| bad("a frame longer than 4 GiB"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&[kind])?;
    w.write_all(&seq.to_le_bytes())?;
    w.write_all(payload)
}

/// The next frame, or None when the stream ends between frames.
fn read_frame(r: &mut impl Read) -> io::Result<Option<(u8, u32, Vec<u8>)>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < len.len() {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the stream ended inside a frame")),
            Ok(n) => got += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    let len = u32::from_le_bytes(len) as usize;
    if !(5..=FRAME_LIMIT).contains(&len) {
        return Err(bad(&format!("a frame of {len} bytes")));
    }
    let mut frame = vec![0u8; len];
    r.read_exact(&mut frame)?;
    let seq = u32::from_le_bytes([frame[1], frame[2], frame[3], frame[4]]);
    Ok(Some((frame[0], seq, frame.split_off(5))))
}

/// Reads a payload's fields in order.
struct Fields<'a>(&'a [u8]);

impl Fields<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        if self.0.len() < n {
            return Err(bad("a payload shorter than its fields"));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> io::Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> io::Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> io::Result<u64> {
        let mut b = [0u8; 8];
        b.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(b))
    }
}

fn micros(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

fn encode_steps(steps: &[Step]) -> io::Result<Vec<u8>> {
    let count = u16::try_from(steps.len()).map_err(|_| bad("a sequence of more than 65535 steps"))?;
    let mut payload = count.to_le_bytes().to_vec();
    for step in steps {
        let (tag, value) = match *step {
            Step::Dtr(on) => (STEP_DTR, u64::from(on)),
            Step::Rts(on) => (STEP_RTS, u64::from(on)),
            Step::Wait(d) => (STEP_WAIT, micros(d)),
            Step::Drain => (STEP_DRAIN, 0),
        };
        payload.push(tag);
        payload.extend_from_slice(&value.to_le_bytes());
    }
    Ok(payload)
}

fn decode_steps(payload: &[u8]) -> io::Result<Vec<Step>> {
    let mut fields = Fields(payload);
    let count = fields.u16()?;
    (0..count).map(|_| {
        let (tag, value) = (fields.u8()?, fields.u64()?);
        Ok(match tag {
            STEP_DTR => Step::Dtr(value != 0),
            STEP_RTS => Step::Rts(value != 0),
            STEP_WAIT => Step::Wait(Duration::from_micros(value)),
            STEP_DRAIN => Step::Drain,
            _ => return Err(bad(&format!("sequence step tag {tag}"))),
        })
    }).collect()
}

/// What one op answers.
enum Answer {
    None,
    Frame(u8, Vec<u8>),
    Close,
}

/// The node's side of one session: the open line and what it was opened with.
struct Session<'a> {
    opener: &'a mut dyn Opener,
    line: Option<Box<dyn Line + Send>>,
    mode: Mode,
    baud: u32,
    /// A write's failure, kept for the next op that waits.
    deferred: Option<io::Error>,
}

impl Session<'_> {
    fn op(&mut self, op: u8, payload: &[u8]) -> Answer {
        let result = match op {
            WRITE => {
                if self.deferred.is_none() {
                    if let Err(err) = self.line().and_then(|line| line.write_all(payload)) {
                        self.deferred = Some(err);
                    }
                }
                return Answer::None;
            }
            PING => Ok(Answer::Frame(PONG, Vec::new())),
            CLOSE => return Answer::Close,
            _ => match self.deferred.take() {
                Some(err) => Err(err),
                None => self.waiting(op, payload),
            },
        };
        result.unwrap_or_else(|err| Answer::Frame(ERR, err_payload(&err)))
    }

    fn line(&mut self) -> io::Result<&mut (dyn Line + Send + 'static)> {
        self.line.as_deref_mut().ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "the session's port is not open"))
    }

    /// An op that answers.
    fn waiting(&mut self, op: u8, payload: &[u8]) -> io::Result<Answer> {
        let mut fields = Fields(payload);
        match op {
            OPEN => {
                if self.line.is_some() {
                    return Err(io::Error::other("the session's port is open already"));
                }
                let (mode, baud) = (Mode::of(fields.u8()?)?, fields.u32()?);
                self.line = Some(self.opener.open(mode, baud)?);
                (self.mode, self.baud) = (mode, baud);
                Ok(Answer::Frame(OK, Vec::new()))
            }
            WANT => {
                let timeout = Duration::from_micros(fields.u64()?);
                let line = self.line()?;
                let Some(first) = line.read_byte(timeout)? else { return Ok(Answer::Frame(QUIET, Vec::new())) };
                let mut bytes = vec![first];
                while bytes.len() < WANT_BYTES {
                    match line.read_byte(Duration::ZERO)? {
                        Some(byte) => bytes.push(byte),
                        None => break,
                    }
                }
                Ok(Answer::Frame(DATA, bytes))
            }
            DRAIN => {
                self.line()?.drain_input()?;
                Ok(Answer::Frame(DRAINED, Vec::new()))
            }
            SEQUENCE => {
                let steps = decode_steps(payload)?;
                match self.line()?.sequence(&steps) {
                    Ok(()) => Ok(Answer::Frame(OK, Vec::new())),
                    Err((index, err)) => {
                        let mut failed = u16::try_from(index).unwrap_or(u16::MAX).to_le_bytes().to_vec();
                        failed.extend_from_slice(&err_payload(&err));
                        Ok(Answer::Frame(FAILED, failed))
                    }
                }
            }
            BAUD => {
                let baud = fields.u32()?;
                self.line()?.set_baud(baud)?;
                self.baud = baud;
                Ok(Answer::Frame(OK, Vec::new()))
            }
            REOPEN => {
                let window = Duration::from_micros(fields.u64()?);
                // The old port closes before the device is looked for again.
                self.line = None;
                self.line = Some(self.opener.reopen(self.mode, self.baud, window)?);
                Ok(Answer::Frame(OK, Vec::new()))
            }
            _ => Err(io::Error::new(io::ErrorKind::Unsupported, format!("op {op} is not on serial wire v{VERSION}"))),
        }
    }
}

/// The node end of one session: reads the client's ops from `read`, runs them on the line
/// `opener` opens, and writes the answers to `write`. Returns when the stream ends or the client
/// closes, after the op under way (a sequence runs to its end) and with the port closed. A client
/// of another wire version gets this node's hello and an `err` naming both versions.
pub fn serve(read: impl Read + Send, write: impl Write + Send, opener: &mut dyn Opener) -> io::Result<()> {
    let mut read = BufReader::new(read);
    let mut write = BufWriter::new(write);
    let mut hello = [0u8; 10];
    read.read_exact(&mut hello)?;
    write.write_all(MAGIC)?;
    write.write_all(&VERSION.to_le_bytes())?;
    if &hello[..8] != MAGIC {
        write_frame(&mut write, ERR, 0, &err_payload(&bad("the client sent no serial hello")))?;
        write.flush()?;
        return Err(bad("the client sent no serial hello"));
    }
    let version = u16::from_le_bytes([hello[8], hello[9]]);
    if version != VERSION {
        let err = io::Error::new(io::ErrorKind::Unsupported, format!("the client speaks serial wire v{version}, this node v{VERSION}"));
        write_frame(&mut write, ERR, 0, &err_payload(&err))?;
        write.flush()?;
        return Ok(());
    }
    write.flush()?;
    let mut session = Session { opener, line: None, mode: Mode::Serial, baud: 0, deferred: None };
    while let Some((op, seq, payload)) = read_frame(&mut read)? {
        match session.op(op, &payload) {
            Answer::None => {}
            Answer::Frame(kind, payload) => {
                write_frame(&mut write, kind, seq, &payload)?;
                write.flush()?;
            }
            Answer::Close => break,
        }
    }
    session.line = None;
    Ok(())
}

/// A session the node or its bridge will not run (the device is busy, its claim is gone, it is
/// off now): reads the client's hello, answers it, and answers the client's first op with `err`
/// of `kind` and `text`, so the client reports that reason as its own error, not a dropped link.
/// A client of another wire version gets the version error instead, as from [`serve`].
pub fn refuse(read: impl Read + Send, write: impl Write + Send, kind: io::ErrorKind, text: &str) -> io::Result<()> {
    let mut read = BufReader::new(read);
    let mut write = BufWriter::new(write);
    let mut hello = [0u8; 10];
    read.read_exact(&mut hello)?;
    write.write_all(MAGIC)?;
    write.write_all(&VERSION.to_le_bytes())?;
    let version = u16::from_le_bytes([hello[8], hello[9]]);
    let (seq, err) = if &hello[..8] != MAGIC {
        (0, bad("the client sent no serial hello"))
    } else if version != VERSION {
        (0, io::Error::new(io::ErrorKind::Unsupported, format!("the client speaks serial wire v{version}, this node v{VERSION}")))
    } else {
        write.flush()?;
        let Some((_, seq, _)) = read_frame(&mut read)? else { return Ok(()) };
        (seq, io::Error::new(kind, text.to_owned()))
    };
    write_frame(&mut write, ERR, seq, &err_payload(&err))?;
    write.flush()
}

/// The link a [`RemoteLine`] runs over: a byte stream whose reads can be bounded in time.
pub trait Link: Read + Write + Send {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
}

#[cfg(unix)]
impl Link for std::os::unix::net::UnixStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        std::os::unix::net::UnixStream::set_read_timeout(self, timeout)
    }
}

/// A serial line on another PC, through its node's [`serve`]. `peer` names that PC or device in
/// errors ("the link to <peer> dropped").
pub struct RemoteLine<S: Link> {
    stream: BufReader<S>,
    seq: u32,
    /// Bytes a `data` answer brought beyond what was read.
    buffer: VecDeque<u8>,
    peer: String,
    /// Set once the link fails: every later op reports it.
    dropped: Option<String>,
    /// An error the node answered to an op that returns none (`sleep`'s wait): the next op
    /// reports it.
    kept: Option<io::Error>,
}

impl<S: Link> RemoteLine<S> {
    /// Says hello over `stream` and opens the node's device as `mode` at `baud`.
    pub fn open(stream: S, peer: &str, mode: Mode, baud: u32) -> io::Result<Self> {
        let mut line = RemoteLine {
            stream: BufReader::new(stream), seq: 0, buffer: VecDeque::new(), peer: peer.to_owned(), dropped: None, kept: None,
        };
        let mut hello = MAGIC.to_vec();
        hello.extend_from_slice(&VERSION.to_le_bytes());
        line.stream.get_mut().write_all(&hello).and_then(|()| line.stream.get_mut().flush()).map_err(|err| line.drop_link(&err))?;
        line.stream.get_ref().set_read_timeout(Some(LINK_SLACK))?;
        let mut answer = [0u8; 10];
        line.stream.read_exact(&mut answer).map_err(|err| line.drop_link(&err))?;
        if &answer[..8] != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{} answered without a serial hello", line.peer)));
        }
        let version = u16::from_le_bytes([answer[8], answer[9]]);
        if version != VERSION {
            let why = match read_frame(&mut line.stream) {
                Ok(Some((ERR, _, payload))) => String::from_utf8_lossy(payload.get(1..).unwrap_or(&[])).into_owned(),
                _ => format!("this client speaks serial wire v{VERSION}, the node v{version}"),
            };
            return Err(io::Error::new(io::ErrorKind::Unsupported, format!("{}: {why}", line.peer)));
        }
        let mut payload = vec![mode.code()];
        payload.extend_from_slice(&baud.to_le_bytes());
        line.call(OPEN, &payload, Duration::ZERO)?;
        Ok(line)
    }

    /// A round trip that leaves the port untouched: the node is there.
    pub fn ping(&mut self) -> io::Result<()> {
        self.call(PING, &[], Duration::ZERO).map(drop)
    }

    fn drop_link(&mut self, err: &io::Error) -> io::Error {
        let why = match err.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut =>
                format!("the link to {} went silent for {} s past its op: flash again", self.peer, LINK_SLACK.as_secs()),
            _ => format!("the link to {} dropped ({err}): flash again", self.peer),
        };
        self.dropped = Some(why.clone());
        io::Error::new(io::ErrorKind::ConnectionAborted, why)
    }

    fn check(&mut self) -> io::Result<()> {
        if let Some(why) = &self.dropped {
            return Err(io::Error::new(io::ErrorKind::ConnectionAborted, why.clone()));
        }
        self.kept.take().map_or(Ok(()), Err)
    }

    fn send(&mut self, op: u8, payload: &[u8]) -> io::Result<u32> {
        self.check()?;
        self.seq += 1;
        let seq = self.seq;
        let sent = write_frame(self.stream.get_mut(), op, seq, payload).and_then(|()| self.stream.get_mut().flush());
        sent.map_err(|err| self.drop_link(&err))?;
        Ok(seq)
    }

    /// Sends `op` and waits for its answer, which may take `time` on the node; an `err` answer
    /// is the error it names.
    fn call(&mut self, op: u8, payload: &[u8], time: Duration) -> io::Result<(u8, Vec<u8>)> {
        let seq = self.send(op, payload)?;
        // Past a day the wait is a block on the stream, which a bridge's drop still ends.
        let wait = time.saturating_add(LINK_SLACK);
        let bound = (wait < Duration::from_secs(86_400)).then_some(wait);
        self.stream.get_ref().set_read_timeout(bound).map_err(|err| self.drop_link(&err))?;
        loop {
            let frame = read_frame(&mut self.stream).map_err(|err| self.drop_link(&err))?;
            let Some((kind, at, payload)) = frame else {
                return Err(self.drop_link(&io::Error::new(io::ErrorKind::UnexpectedEof, "the stream ended")));
            };
            if kind == PUSH {
                self.buffer.extend(payload);
                continue;
            }
            if at != seq {
                return Err(self.drop_link(&bad(&format!("an answer to op {at} while op {seq} waits"))));
            }
            if kind == ERR {
                let mut fields = Fields(&payload);
                let code = fields.u8().map_err(|err| self.drop_link(&err))?;
                return Err(io::Error::new(err_kind(code), format!("{}: {}", self.peer, String::from_utf8_lossy(fields.0))));
            }
            return Ok((kind, payload));
        }
    }

    fn expect(&mut self, op: u8, payload: &[u8], time: Duration, want: u8) -> io::Result<Vec<u8>> {
        let (kind, payload) = self.call(op, payload, time)?;
        if kind != want {
            return Err(self.drop_link(&bad(&format!("answer kind {kind} to op {op}"))));
        }
        Ok(payload)
    }
}

impl<S: Link> Line for RemoteLine<S> {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        for chunk in bytes.chunks(WRITE_BYTES) {
            self.send(WRITE, chunk)?;
        }
        Ok(())
    }

    fn read_byte(&mut self, timeout: Duration) -> io::Result<Option<u8>> {
        if let Some(byte) = self.buffer.pop_front() {
            return Ok(Some(byte));
        }
        let (kind, payload) = self.call(WANT, &micros(timeout).to_le_bytes(), timeout)?;
        match kind {
            QUIET => Ok(None),
            DATA if !payload.is_empty() => {
                self.buffer.extend(payload);
                Ok(self.buffer.pop_front())
            }
            _ => Err(self.drop_link(&bad(&format!("answer kind {kind} to a want")))),
        }
    }

    fn set_dtr(&mut self, on: bool) -> io::Result<()> {
        self.sequence(&[Step::Dtr(on)]).map_err(|(_, err)| err)
    }

    fn set_rts(&mut self, on: bool) -> io::Result<()> {
        self.sequence(&[Step::Rts(on)]).map_err(|(_, err)| err)
    }

    fn set_baud(&mut self, baud: u32) -> io::Result<()> {
        self.expect(BAUD, &baud.to_le_bytes(), Duration::ZERO, OK).map(drop)
    }

    fn drain_input(&mut self) -> io::Result<()> {
        self.buffer.clear();
        self.expect(DRAIN, &[], Duration::ZERO, DRAINED).map(drop)
    }

    /// Sent whole: the node runs every step, its waits on its own clock. A link failure is
    /// reported at index 0, as nothing here knows how far the node got.
    fn sequence(&mut self, steps: &[Step]) -> Result<(), (usize, io::Error)> {
        if steps.contains(&Step::Drain) {
            self.buffer.clear();
        }
        let payload = encode_steps(steps).map_err(|err| (0, err))?;
        let time = steps.iter().map(|step| match step { Step::Wait(d) => *d, _ => Duration::ZERO }).fold(Duration::ZERO, Duration::saturating_add);
        let (kind, payload) = self.call(SEQUENCE, &payload, time).map_err(|err| (0, err))?;
        match kind {
            OK => Ok(()),
            FAILED => {
                let mut fields = Fields(&payload);
                let parsed = (|| Ok::<_, io::Error>((fields.u16()?, fields.u8()?)))();
                let (index, code) = parsed.map_err(|err| (0, self.drop_link(&err)))?;
                let text = String::from_utf8_lossy(fields.0);
                Err((usize::from(index), io::Error::new(err_kind(code), format!("{}: {text}", self.peer))))
            }
            _ => Err((0, self.drop_link(&bad(&format!("answer kind {kind} to a sequence"))))),
        }
    }

    fn reopen(&mut self) -> io::Result<()> {
        self.buffer.clear();
        self.expect(REOPEN, &micros(CONNECT_WINDOW).to_le_bytes(), CONNECT_WINDOW, OK).map(drop)
    }

    /// On the node's clock, in order with the writes before it. An error the node answers (a
    /// write's, kept there) is kept for the next op, which reports it; a dropped link reports
    /// itself.
    fn sleep(&mut self, d: Duration) {
        if let Err((_, err)) = self.sequence(&[Step::Wait(d)]) {
            if self.dropped.is_none() {
                self.kept = Some(err);
            }
        }
    }
}

impl<S: Link> Drop for RemoteLine<S> {
    fn drop(&mut self) {
        if self.dropped.is_none() {
            // The node closes the port, then the stream; nothing waits for it.
            let _ = self.send(CLOSE, &[]);
        }
    }
}
