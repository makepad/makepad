//! A connection: numbered channels over one carrier, each with credit flow control.
//!
//! A reader thread parses frames and routes them (DATA to the channel's consumer, CREDIT to the
//! sender state, OPEN to the endpoint's service factories). A writer thread owns the carrier's
//! write side and drains the per-channel queues in priority order (control > cmd > watch > input >
//! sync > surface > frames > stream), sending a channel's data only while it has credit. An app
//! thread never blocks on the socket: [`ChannelTx::try_send`] returns [`SendError::Full`] and the
//! service decides what to drop.

use crate::frame::{kind, write_frame, Frame, FrameReader, FLAG_MORE, MAX_PAYLOAD};
use crate::hello::{self, Hello, Resume};
use std::collections::{HashMap, VecDeque};
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// Bytes a sender may have outstanding on a channel before the receiver grants more.
pub const INITIAL_CREDIT: usize = 256 << 10;
/// The receiver grants after the consumer has drained this much.
const GRANT_AT: usize = 64 << 10;
/// Bytes queued per channel before `try_send` says Full (a single larger message is accepted when
/// the queue is empty).
pub const QUEUE_CAP: usize = 1 << 20;
const KEEPALIVE: Duration = Duration::from_secs(5);

pub const CLOSE_NORMAL: u16 = 0;
pub const CLOSE_UNSUPPORTED: u16 = 1;
pub const CLOSE_PROTOCOL: u16 = 2;
pub const CLOSE_REFUSED: u16 = 3;
pub const CLOSE_LINK_LOST: u16 = 4;

/// Nanoseconds since the first call in this process (the clock PING/PONG use).
pub fn mono_ns() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64
}

/// Send priority of a service's channels: lower goes first.
pub fn priority(service: &str) -> u8 {
    match service {
        "cmd" => 1,
        "watch" => 2,
        "input" => 3,
        "sync" => 4,
        "surface" => 5,
        "frames" => 6,
        _ => 7,
    }
}

#[derive(Debug)]
pub enum Event {
    /// the peer accepted our OPEN (the service's caps)
    Opened(String),
    Data(Vec<u8>),
    Closed(u16, String),
}

#[derive(Debug)]
pub enum SendError {
    /// The queue is full; the message comes back so the service can drop or retry it.
    Full(Vec<u8>),
    Closed,
}

/// Runs first / last on every thread a connection starts (a host excludes them from its own
/// sampling).
#[derive(Clone, Copy)]
pub struct Hooks {
    pub enter: fn(),
    pub exit: fn(),
}

impl Default for Hooks {
    fn default() -> Self {
        fn nop() {}
        Hooks { enter: nop, exit: nop }
    }
}

/// The two halves of a carrier, handshake done.
pub struct Io {
    pub reader: FrameReader,
    pub writer: Box<dyn Write + Send>,
    /// closes the carrier, waking a reader blocked in it
    pub shutdown: Box<dyn Fn() + Send + Sync>,
}

/// An OPEN the peer sent.
pub struct Open {
    pub service: String,
    pub args: String,
    pub channel: Channel,
}

/// Decides an incoming OPEN: `Ok(caps)` answers OK, `Err((code, reason))` answers CLOSE. The
/// handler keeps the channel (it usually hands it to a thread).
pub type OpenHandler = Arc<dyn Fn(&Conn, Open) -> Result<String, (u16, String)> + Send + Sync>;

struct Chan {
    prio: u8,
    credit: usize,
    q: VecDeque<Vec<u8>>,
    off: usize,
    queued: usize,
    tx: Sender<(Event, bool)>,
    ungranted: usize,
    served: u64,
    reasm: Vec<u8>,
    partial: bool,
    /// opened by the peer and not yet answered: nothing is written for it until the OK or CLOSE
    hold: bool,
    close_after: Option<(u16, String)>,
}

#[derive(Default)]
struct State {
    ctrl: VecDeque<Frame>,
    chans: HashMap<u16, Chan>,
    served: u64,
    next_id: u16,
    draining: bool,
}

struct Shared {
    st: Mutex<State>,
    work: Condvar,
    space: Condvar,
    closed: Arc<AtomicBool>,
    /// close() was called: the goodbye is being written, no new work should start
    closing: AtomicBool,
    peer: Hello,
    resume: Mutex<Option<Resume>>,
    rtt_ns: AtomicU64,
    reason: Mutex<String>,
    shutdown: Box<dyn Fn() + Send + Sync>,
    handler: Option<OpenHandler>,
    hooks: Hooks,
}

#[derive(Clone)]
pub struct Conn(Arc<Shared>);

enum Work {
    Frame(Frame),
    Flush,
    Exit,
}

impl State {
    fn push_ctrl(&mut self, chan: u16, k: u8, payload: Vec<u8>) {
        self.ctrl.push_back(Frame { chan, kind: k, flags: 0, payload });
    }

    fn pick(&mut self) -> Option<Frame> {
        if let Some(f) = self.ctrl.pop_front() {
            return Some(f);
        }
        let mut best: Option<(u8, u64, u16)> = None;
        for (id, c) in &self.chans {
            let ready = !c.hold && match c.q.front() {
                Some(m) => c.credit > 0 || (m.is_empty() && c.off == 0),
                None => c.close_after.is_some(),
            };
            if ready {
                let key = (if c.q.is_empty() { 0 } else { c.prio }, c.served, *id);
                if best.is_none_or(|b| key < b) {
                    best = Some(key);
                }
            }
        }
        let (_, _, id) = best?;
        self.served += 1;
        let served = self.served;
        let c = self.chans.get_mut(&id)?;
        c.served = served;
        let Some(msg) = c.q.front() else {
            let (code, reason) = c.close_after.take().unwrap_or((CLOSE_NORMAL, String::new()));
            self.chans.remove(&id);
            let mut p = code.to_le_bytes().to_vec();
            p.extend_from_slice(reason.as_bytes());
            return Some(Frame { chan: id, kind: kind::CLOSE, flags: 0, payload: p });
        };
        let n = (msg.len() - c.off).min(c.credit).min(MAX_PAYLOAD);
        let payload;
        let more;
        if c.off == 0 && n == msg.len() {
            payload = c.q.pop_front().unwrap_or_default();
            more = false;
        } else {
            payload = msg[c.off..c.off + n].to_vec();
            c.off += n;
            more = c.off < msg.len();
            if !more {
                c.q.pop_front();
                c.off = 0;
            }
        }
        c.credit -= n;
        c.queued -= n;
        Some(Frame { chan: id, kind: kind::DATA, flags: if more { FLAG_MORE } else { 0 }, payload })
    }
}

impl Conn {
    /// Starts the reader and writer threads over `io`, after the handshake. `initiator`: the
    /// side that dialled (it allocates odd channel numbers, the other even).
    pub fn start(io: Io, peer: Hello, initiator: bool, handler: Option<OpenHandler>, hooks: Hooks) -> Conn {
        let closed = Arc::new(AtomicBool::new(false));
        let mut reader = io.reader;
        reader.cancel = Some(closed.clone());
        reader.deadline = None;
        let shared = Arc::new(Shared {
            st: Mutex::new(State { next_id: if initiator { 1 } else { 2 }, ..State::default() }),
            work: Condvar::new(),
            space: Condvar::new(),
            closed,
            closing: AtomicBool::new(false),
            peer,
            resume: Mutex::new(None),
            rtt_ns: AtomicU64::new(0),
            reason: Mutex::new(String::new()),
            shutdown: io.shutdown,
            handler,
            hooks,
        });
        let conn = Conn(shared);
        let (c1, c2) = (conn.clone(), conn.clone());
        let writer = io.writer;
        let _ = std::thread::Builder::new().name("makepad-remote-rx".into()).spawn(move || {
            (c1.0.hooks.enter)();
            c1.read_loop(reader);
            (c1.0.hooks.exit)();
        });
        let _ = std::thread::Builder::new().name("makepad-remote-tx".into()).spawn(move || {
            (c2.0.hooks.enter)();
            c2.write_loop(writer);
            (c2.0.hooks.exit)();
        });
        conn
    }

    pub fn hooks(&self) -> Hooks {
        self.0.hooks
    }

    pub fn peer(&self) -> &Hello {
        &self.0.peer
    }

    pub fn is_closed(&self) -> bool {
        self.0.closed.load(Relaxed)
    }

    /// False once the connection has ended or `close` was called (a client dials a new one).
    pub fn is_usable(&self) -> bool {
        !self.is_closed() && !self.0.closing.load(Relaxed)
    }

    /// Blocks until the connection has ended.
    pub fn wait_closed(&self) {
        let mut st = self.0.st.lock().unwrap();
        while !self.is_closed() {
            st = self.0.space.wait(st).unwrap();
        }
    }

    /// Why the connection ended (empty while it is open).
    pub fn close_reason(&self) -> String {
        self.0.reason.lock().unwrap().clone()
    }

    /// Round trip of the last PING, ns (0 before the first PONG).
    pub fn rtt_ns(&self) -> u64 {
        self.0.rtt_ns.load(Relaxed)
    }

    /// The RESUME the peer sent, if any.
    pub fn resume(&self) -> Option<Resume> {
        self.0.resume.lock().unwrap().clone()
    }

    pub fn ping(&self) {
        self.ctrl(0, kind::PING, mono_ns().to_le_bytes().to_vec());
    }

    /// Sends a channel-0 control message (see `hello`).
    pub fn send_control(&self, payload: Vec<u8>) {
        self.ctrl(0, kind::DATA, payload);
    }

    fn ctrl(&self, chan: u16, k: u8, payload: Vec<u8>) {
        if self.is_closed() {
            return;
        }
        self.0.st.lock().unwrap().push_ctrl(chan, k, payload);
        self.0.work.notify_one();
    }

    /// Says goodbye and closes once everything queued before it is written.
    pub fn close(&self, reason: &str) {
        self.0.closing.store(true, Relaxed);
        let mut st = self.0.st.lock().unwrap();
        st.push_ctrl(0, kind::DATA, hello::encode_goodbye(reason));
        st.draining = true;
        drop(st);
        self.0.work.notify_one();
    }

    fn terminate(&self, reason: &str) {
        if self.0.closed.swap(true, Relaxed) {
            return;
        }
        *self.0.reason.lock().unwrap() = reason.to_string();
        let chans = std::mem::take(&mut self.0.st.lock().unwrap().chans);
        for (_, c) in chans {
            let _ = c.tx.send((Event::Closed(CLOSE_LINK_LOST, reason.to_string()), false));
        }
        self.0.work.notify_all();
        self.0.space.notify_all();
        (self.0.shutdown)();
    }

    /// Opens a channel against `service` and waits for the peer's answer.
    pub fn open(&self, service: &str, args: &str) -> Result<Channel, (u16, String)> {
        self.open_timeout(service, args, Duration::from_secs(10))
    }

    pub fn open_timeout(&self, service: &str, args: &str, wait: Duration) -> Result<Channel, (u16, String)> {
        if !self.is_usable() {
            return Err((CLOSE_LINK_LOST, self.close_reason()));
        }
        let (tx, rx) = channel();
        let id = {
            let mut st = self.0.st.lock().unwrap();
            let mut id = st.next_id;
            let mut tries = 0u32;
            while st.chans.contains_key(&id) || id == 0 {
                id = id.wrapping_add(2);
                tries += 1;
                if tries > 40000 {
                    return Err((CLOSE_REFUSED, "no free channel number".into()));
                }
            }
            st.next_id = id.wrapping_add(2);
            st.chans.insert(id, new_chan(service, tx));
            st.push_ctrl(id, kind::OPEN, format!("{service}\n{args}").into_bytes());
            id
        };
        self.0.work.notify_one();
        let ch = Channel { tx: ChannelTx { conn: self.clone(), id }, rx };
        match ch.rx.recv_timeout(wait) {
            Ok((Event::Opened(_), _)) => Ok(ch),
            Ok((Event::Closed(code, reason), _)) => Err((code, reason)),
            Ok((Event::Data(_), _)) => Err((CLOSE_PROTOCOL, "data before OK".into())),
            Err(_) => Err((CLOSE_LINK_LOST, "no answer to OPEN".into())),
        }
    }

    // ---- reader

    fn read_loop(&self, mut rd: FrameReader) {
        loop {
            let f = match rd.next() {
                Ok(f) => f,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::InvalidData {
                        // tell the peer why before closing
                        self.close(&e.to_string());
                    } else {
                        self.terminate(&e.to_string());
                    }
                    return;
                }
            };
            if let Err(msg) = self.on_frame(f) {
                self.close(&msg);
                return;
            }
            if self.is_closed() {
                return;
            }
        }
    }

    fn on_frame(&self, f: Frame) -> Result<(), String> {
        match f.kind {
            kind::PING => self.ctrl(0, kind::PONG, f.payload),
            kind::PONG => {
                if let Ok(b) = <[u8; 8]>::try_from(f.payload.as_slice()) {
                    self.0.rtt_ns.store(mono_ns().saturating_sub(u64::from_le_bytes(b)), Relaxed);
                }
            }
            kind::DATA if f.chan == 0 => self.on_control(&f.payload),
            kind::OPEN => self.on_open(f)?,
            kind::OK => {
                let st = self.0.st.lock().unwrap();
                if let Some(c) = st.chans.get(&f.chan) {
                    let _ = c.tx.send((Event::Opened(String::from_utf8_lossy(&f.payload).into_owned()), false));
                }
            }
            kind::DATA => {
                let mut st = self.0.st.lock().unwrap();
                let mut credit = None;
                if let Some(c) = st.chans.get_mut(&f.chan) {
                    // the pieces of a long message are credited as they arrive: the consumer
                    // sees the message only whole, and the sender needs the credit to finish it
                    let n = f.payload.len();
                    if f.flags & FLAG_MORE != 0 {
                        c.reasm.extend_from_slice(&f.payload);
                        c.partial = true;
                        credit = Some(n);
                    } else if c.partial {
                        c.reasm.extend_from_slice(&f.payload);
                        c.partial = false;
                        let data = std::mem::take(&mut c.reasm);
                        let _ = c.tx.send((Event::Data(data), true));
                        credit = Some(n);
                    } else {
                        let _ = c.tx.send((Event::Data(f.payload), false));
                    }
                }
                if let Some(n) = credit {
                    Self::credit(&mut st, f.chan, n);
                }
                drop(st);
                self.0.work.notify_one();
            }
            kind::CREDIT => {
                if let Ok(b) = <[u8; 4]>::try_from(f.payload.as_slice()) {
                    let mut st = self.0.st.lock().unwrap();
                    if let Some(c) = st.chans.get_mut(&f.chan) {
                        c.credit += u32::from_le_bytes(b) as usize;
                        drop(st);
                        self.0.work.notify_one();
                    }
                }
            }
            kind::CLOSE => {
                let mut st = self.0.st.lock().unwrap();
                if let Some(c) = st.chans.remove(&f.chan) {
                    let code = f.payload.get(..2).map_or(CLOSE_NORMAL, |b| u16::from_le_bytes([b[0], b[1]]));
                    let reason = f.payload.get(2..).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
                    let _ = c.tx.send((Event::Closed(code, reason), false));
                    drop(st);
                    self.0.space.notify_all();
                }
            }
            // HANDLE is reserved, and a newer minor version may add kinds: both are skipped
            _ => {}
        }
        Ok(())
    }

    fn on_control(&self, p: &[u8]) {
        match p.first() {
            Some(&hello::OP_RESUME) => *self.0.resume.lock().unwrap() = Some(hello::decode_resume(&p[1..])),
            Some(&hello::OP_GOODBYE) => self.terminate(&format!("peer closed: {}", String::from_utf8_lossy(&p[1..]))),
            _ => {}
        }
    }

    fn on_open(&self, f: Frame) -> Result<(), String> {
        let text = String::from_utf8_lossy(&f.payload).into_owned();
        let (service, args) = text.split_once('\n').map_or((text.as_str(), ""), |(a, b)| (a, b));
        let (tx, rx) = channel();
        {
            let mut st = self.0.st.lock().unwrap();
            if f.chan == 0 || st.chans.contains_key(&f.chan) {
                return Err(format!("OPEN of channel {} which is already in use", f.chan));
            }
            let mut c = new_chan(service, tx);
            c.hold = true;
            st.chans.insert(f.chan, c);
        }
        let channel = Channel { tx: ChannelTx { conn: self.clone(), id: f.chan }, rx };
        let open = Open { service: service.to_string(), args: args.to_string(), channel };
        let answer = match &self.0.handler {
            Some(h) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h(self, open))).unwrap_or_else(|_| Err((CLOSE_REFUSED, "the service failed to open".into()))),
            None => Err((CLOSE_UNSUPPORTED, format!("unsupported service {service}"))),
        };
        let mut st = self.0.st.lock().unwrap();
        match answer {
            Ok(caps) => {
                if let Some(c) = st.chans.get_mut(&f.chan) {
                    c.hold = false;
                }
                st.push_ctrl(f.chan, kind::OK, caps.into_bytes());
            }
            Err((code, reason)) => {
                st.chans.remove(&f.chan);
                let mut p = code.to_le_bytes().to_vec();
                p.extend_from_slice(reason.as_bytes());
                st.push_ctrl(f.chan, kind::CLOSE, p);
            }
        }
        drop(st);
        self.0.work.notify_one();
        Ok(())
    }

    // ---- writer

    fn write_loop(&self, w: Box<dyn Write + Send>) {
        let mut w = BufWriter::with_capacity(64 << 10, w);
        let mut flushed = true;
        let mut idle_since = std::time::Instant::now();
        loop {
            let work = {
                let mut st = self.0.st.lock().unwrap();
                loop {
                    if self.is_closed() {
                        return;
                    }
                    if let Some(f) = st.pick() {
                        break Work::Frame(f);
                    }
                    if !flushed {
                        break Work::Flush;
                    }
                    if st.draining {
                        break Work::Exit;
                    }
                    let (g, to) = self.0.work.wait_timeout(st, KEEPALIVE).unwrap();
                    st = g;
                    if to.timed_out() && idle_since.elapsed() >= KEEPALIVE {
                        st.push_ctrl(0, kind::PING, mono_ns().to_le_bytes().to_vec());
                    }
                }
            };
            let r = match work {
                Work::Frame(f) => {
                    flushed = false;
                    idle_since = std::time::Instant::now();
                    let r = write_frame(&mut w, f.chan, f.kind, f.flags, &f.payload);
                    if f.kind == kind::DATA {
                        self.0.space.notify_all();
                    }
                    r
                }
                Work::Flush => {
                    flushed = true;
                    w.flush()
                }
                Work::Exit => {
                    let _ = w.flush();
                    self.terminate("closed");
                    return;
                }
            };
            if let Err(e) = r {
                self.terminate(&format!("write failed: {e}"));
                return;
            }
        }
    }

    fn grant(&self, id: u16, n: usize) {
        let mut st = self.0.st.lock().unwrap();
        Self::credit(&mut st, id, n);
        drop(st);
        self.0.work.notify_one();
    }

    /// Counts `n` consumed bytes of channel `id`; hands the sender more credit past a quarter window.
    fn credit(st: &mut State, id: u16, n: usize) {
        let Some(c) = st.chans.get_mut(&id) else { return };
        c.ungranted += n;
        if c.ungranted >= GRANT_AT {
            let g = std::mem::take(&mut c.ungranted) as u32;
            st.push_ctrl(id, kind::CREDIT, g.to_le_bytes().to_vec());
        }
    }
}

fn new_chan(service: &str, tx: Sender<(Event, bool)>) -> Chan {
    Chan { prio: priority(service), credit: INITIAL_CREDIT, q: VecDeque::new(), off: 0, queued: 0, tx, ungranted: 0, served: 0, reasm: Vec::new(), partial: false, hold: false, close_after: None }
}

/// The sending half of a channel; cloneable, usable from any thread.
#[derive(Clone)]
pub struct ChannelTx {
    conn: Conn,
    id: u16,
}

impl ChannelTx {
    pub fn id(&self) -> u16 {
        self.id
    }

    pub fn conn(&self) -> &Conn {
        &self.conn
    }

    /// Queues `msg` unless the queue is full (never blocks).
    pub fn try_send(&self, msg: Vec<u8>) -> Result<(), SendError> {
        let mut st = self.conn.0.st.lock().unwrap();
        let Some(c) = st.chans.get_mut(&self.id) else { return Err(SendError::Closed) };
        if c.close_after.is_some() {
            return Err(SendError::Closed);
        }
        if c.queued > 0 && c.queued + msg.len() > QUEUE_CAP {
            return Err(SendError::Full(msg));
        }
        c.queued += msg.len();
        c.q.push_back(msg);
        drop(st);
        self.conn.0.work.notify_one();
        Ok(())
    }

    /// Queues `msg`, waiting for room (for service threads; not for app threads).
    pub fn send(&self, msg: Vec<u8>) -> std::io::Result<()> {
        let broken = || std::io::Error::new(std::io::ErrorKind::BrokenPipe, "channel closed");
        let mut st = self.conn.0.st.lock().unwrap();
        loop {
            if self.conn.is_closed() {
                return Err(broken());
            }
            let Some(c) = st.chans.get_mut(&self.id) else { return Err(broken()) };
            if c.close_after.is_some() {
                return Err(broken());
            }
            if c.queued == 0 || c.queued + msg.len() <= QUEUE_CAP {
                c.queued += msg.len();
                c.q.push_back(msg);
                drop(st);
                self.conn.0.work.notify_one();
                return Ok(());
            }
            st = self.conn.0.space.wait(st).unwrap();
        }
    }

    /// Closes the channel once what is queued has been sent.
    pub fn close(&self, code: u16, reason: &str) {
        let mut st = self.conn.0.st.lock().unwrap();
        if let Some(c) = st.chans.get_mut(&self.id) {
            if c.close_after.is_none() {
                c.close_after = Some((code, reason.to_string()));
            }
        }
        drop(st);
        self.conn.0.work.notify_one();
        self.conn.0.space.notify_all();
    }
}

/// One channel: send through [`ChannelTx`] (also available on the channel itself), receive here.
/// Receiving data grants the sender more credit.
pub struct Channel {
    tx: ChannelTx,
    /// (event, its credit was already granted when its fragments arrived)
    rx: Receiver<(Event, bool)>,
}

impl Channel {
    pub fn id(&self) -> u16 {
        self.tx.id
    }

    pub fn tx(&self) -> ChannelTx {
        self.tx.clone()
    }

    pub fn conn(&self) -> &Conn {
        &self.tx.conn
    }

    pub fn try_send(&self, msg: Vec<u8>) -> Result<(), SendError> {
        self.tx.try_send(msg)
    }

    pub fn send(&self, msg: Vec<u8>) -> std::io::Result<()> {
        self.tx.send(msg)
    }

    pub fn close(&self, code: u16, reason: &str) {
        self.tx.close(code, reason)
    }

    fn got(&self, (e, granted): (Event, bool)) -> Event {
        if let (Event::Data(d), false) = (&e, granted) {
            self.tx.conn.grant(self.tx.id, d.len());
        }
        e
    }

    /// The next event; None once the channel is gone.
    pub fn recv(&self) -> Option<Event> {
        self.rx.recv().ok().map(|e| self.got(e))
    }

    /// `Err(true)`: timed out; `Err(false)`: the channel is gone.
    pub fn recv_timeout(&self, d: Duration) -> Result<Event, bool> {
        match self.rx.recv_timeout(d) {
            Ok(e) => Ok(self.got(e)),
            Err(RecvTimeoutError::Timeout) => Err(true),
            Err(RecvTimeoutError::Disconnected) => Err(false),
        }
    }

    pub fn try_recv(&self) -> Result<Event, bool> {
        match self.rx.try_recv() {
            Ok(e) => Ok(self.got(e)),
            Err(TryRecvError::Empty) => Err(true),
            Err(TryRecvError::Disconnected) => Err(false),
        }
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.tx.close(CLOSE_NORMAL, "");
    }
}
