//! A connection: numbered channels over one carrier, each with credit flow control.
//!
//! A reader thread parses frames and routes them (DATA to the channel's consumer, CREDIT to the
//! sender state, OPEN to the endpoint's service factories). A writer thread owns the carrier's
//! write side and drains the per-channel queues in priority order (control > cmd > watch > input >
//! sync > surface > frames > stream), sending a channel's data only while it has credit. An app
//! thread never blocks on the socket: [`ChannelTx::try_send`] returns [`SendError::Full`] and the
//! service decides what to drop.

use crate::frame::{kind, write_frame, Frame, FrameReader, FLAG_MESSAGE_LIMIT, FLAG_MORE, MAX_PAYLOAD};
use crate::hello::{self, Hello, Resume};
use crate::endpoint::{service_limits, ServiceLimits};
use std::collections::{HashMap, VecDeque};
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

/// Bytes a sender may have outstanding on a channel before the receiver grants more.
pub const INITIAL_CREDIT: usize = 256 << 10;
/// The receiver grants after the consumer has drained this much.
const GRANT_AT: usize = 64 << 10;
/// Bytes queued per channel before `try_send` says Full (a single larger message is accepted when
/// the queue is empty).
pub const QUEUE_CAP: usize = 1 << 20;
/// How long a connection's writer stays idle before it pings the peer.
pub const KEEPALIVE: Duration = Duration::from_secs(5);

/// Every connection of this process that has not ended yet ([`shut_down_all`]).
static LIVE: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());

/// Shuts down every connection of this process, both ways, before it execs in place. The exec
/// closes this process's descriptors, but a child that inherited a copy of a connection's socket
/// holds that socket open, and its peer waited on it for good: macOS sets close-on-exec on an
/// accepted or dialled socket only once accept or socket returns, and a spawn on another thread
/// in between inherits it (11-23). Shut down, the copy holds a dead socket and the peer sees the
/// end at once, as at any close; its calls fail as lost and dial again. An exec that fails leaves
/// them shut down: their peers dial again.
pub fn shut_down_all() {
    let live: Vec<Arc<Shared>> = LIVE.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(Weak::upgrade).collect();
    for c in live {
        (c.shutdown)();
    }
}

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

#[derive(Clone)]
pub(crate) struct ServiceOpen {
    pub limits: ServiceLimits,
    pub handler: OpenHandler,
}

pub(crate) type ServiceLookup = Arc<dyn Fn(&str) -> Option<ServiceOpen> + Send + Sync>;

const STRICT_SLICE: usize = 16 << 10;
/// Strict links answer at most four PINGs per second, with no accumulated burst allowance.
const PONG_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy)]
pub struct ConnOptions {
    pub(crate) strict: bool,
    frame_slice: usize,
    pub(crate) open_max_message: fn(&str) -> usize,
}

impl Default for ConnOptions {
    fn default() -> Self {
        // A local link (same user, same machine) carries whatever its services send: a whole file,
        // a big frame. Message ceilings would only surface as random failures there, so they
        // belong to the strict profile alone.
        Self { strict: false, frame_slice: MAX_PAYLOAD, open_max_message: |_| usize::MAX }
    }
}

impl ConnOptions {
    /// For a link whose peer may be on another person's machine.
    pub fn strict() -> Self {
        Self { strict: true, frame_slice: STRICT_SLICE, open_max_message: |name| service_limits(name).max_message }
    }

    /// At most this many DATA bytes per scheduling decision, up to the profile's frame limit.
    pub fn with_frame_slice(mut self, bytes: std::num::NonZeroUsize) -> Self {
        self.frame_slice = bytes.get().min(if self.strict { STRICT_SLICE } else { MAX_PAYLOAD });
        self
    }
}

struct Chan {
    generation: u64,
    prio: u8,
    credit: usize,
    /// Credit still available to the peer for incoming DATA.
    recv_credit: usize,
    q: VecDeque<Queued>,
    off: usize,
    queued: usize,
    tx: Sender<(Event, usize)>,
    ungranted: usize,
    served: u64,
    reasm: Vec<u8>,
    advanced: usize, // reassembly bytes already returned as credit
    unread: usize, // complete DATA bytes still with the receiver
    max_message: usize,
    partial: bool,
    echo: bool,
    /// opened by the peer and not yet answered: nothing is written for it until the OK or CLOSE
    hold: bool,
    awaiting_ok: bool,
    close_after: Option<(u16, String)>,
    // The channel's slot under the service's cap (strict links): freed with this entry, when the
    // CLOSE is sent or received. The service's own end may still be tearing down by then; holding
    // the slot for that would refuse a well-behaved peer that closes and reopens straight away.
    _admission: Option<Arc<str>>,
}

enum Queued {
    Bytes(Vec<u8>),
    // Empty messages use no byte credit. Keep their order and multiplicity in constant space.
    Empty(u64),
}

impl Chan {
    /// Only borrow credit for the current record when the wire window is exhausted.
    /// Once a window of complete records is waiting, its consumer must make room.
    fn advance_partial(&mut self) -> usize {
        if self.partial && !self.echo && self.recv_credit == 0 && self.unread < INITIAL_CREDIT {
            let bytes = self.reasm.len() - self.advanced;
            self.advanced = self.reasm.len();
            return bytes;
        }
        0
    }

    fn enqueue(&mut self, msg: Vec<u8>) {
        if msg.is_empty() {
            if let Some(Queued::Empty(n)) = self.q.back_mut() {
                *n += 1;
            } else {
                self.q.push_back(Queued::Empty(1));
            }
        } else {
            self.queued += msg.len();
            self.q.push_back(Queued::Bytes(msg));
        }
    }
}

#[derive(Default)]
struct State {
    ctrl: VecDeque<Control>,
    chans: HashMap<u16, Chan>,
    generation: u64,
    admissions: Vec<Weak<str>>,
    served: u64,
    next_id: u16,
    draining: bool,
    protocol_error: Option<String>,
    pong: Option<[u8; 8]>,
    last_pong: Option<Instant>,
}

enum Control {
    Frame(Frame),
    Grant { id: u16, generation: u64, bytes: u32 },
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
    lookup: Option<ServiceLookup>,
    options: ConnOptions,
    hooks: Hooks,
    /// how long a ping may go unanswered before the link is lost, ns ([`Conn::set_ping_limit`];
    /// 0: never)
    ping_limit_ns: AtomicU64,
    /// when the oldest ping that nothing came back after went ([`mono_ns`], 0: none is out)
    ping_out_ns: AtomicU64,
}

#[derive(Clone)]
pub struct Conn(Arc<Shared>);

enum Work {
    Frame(Frame),
    Flush,
    Exit,
    /// a ping went unanswered past the limit ([`Conn::set_ping_limit`])
    Lost(String),
    ProtocolError(String),
}

impl State {
    fn push_ctrl(&mut self, chan: u16, k: u8, payload: Vec<u8>) {
        self.push_ctrl_flags(chan, k, 0, payload);
    }

    fn push_ctrl_flags(&mut self, chan: u16, kind: u8, flags: u8, payload: Vec<u8>) {
        self.ctrl.push_back(Control::Frame(Frame { chan, kind, flags, payload }));
    }

    fn insert(&mut self, id: u16, service: &str, max_message: usize, tx: Sender<(Event, usize)>, hold: bool, admission: Option<Arc<str>>) -> u64 {
        self.generation += 1;
        let generation = self.generation;
        self.chans.insert(id, Chan {
            generation, prio: priority(service), credit: INITIAL_CREDIT, recv_credit: INITIAL_CREDIT,
            q: VecDeque::new(), off: 0, queued: 0, tx, ungranted: 0, served: 0,
            reasm: Vec::new(), advanced: 0, unread: 0, max_message, partial: false, echo: false, hold, awaiting_ok: !hold,
            close_after: None, _admission: admission,
        });
        generation
    }

    fn admit(&mut self, service: &str, cap: usize) -> Option<Arc<str>> {
        let mut used = 0;
        self.admissions.retain(|slot| {
            let Some(name) = slot.upgrade() else { return false };
            used += usize::from(&*name == service);
            true
        });
        if used >= cap {
            return None;
        }
        let slot: Arc<str> = service.into();
        self.admissions.push(Arc::downgrade(&slot));
        Some(slot)
    }

    fn push_close(&mut self, id: u16, code: u16, reason: &str) {
        let mut payload = code.to_le_bytes().to_vec();
        payload.extend_from_slice(reason.as_bytes());
        self.push_ctrl(id, kind::CLOSE, payload);
    }

    fn pick(&mut self, slice: usize) -> Option<Frame> {
        if let Some(cookie) = self.pong.take() {
            self.last_pong = Some(Instant::now());
            return Some(Frame { chan: 0, kind: kind::PONG, flags: 0, payload: cookie.to_vec() });
        }
        while let Some(ctrl) = self.ctrl.pop_front() {
            match ctrl {
                Control::Frame(f) => return Some(f),
                Control::Grant { id, generation, bytes } => {
                    // A retired channel's queued grant cannot credit a reuse of its wire id.
                    if let Some(c) = self.chans.get_mut(&id).filter(|c| c.generation == generation) {
                        c.recv_credit += bytes as usize;
                        return Some(Frame { chan: id, kind: kind::CREDIT, flags: 0, payload: bytes.to_le_bytes().to_vec() });
                    }
                }
            }
        }
        let mut best: Option<(u8, u64, u16)> = None;
        for (id, c) in &self.chans {
            let ready = !c.hold && match c.q.front() {
                Some(Queued::Empty(_)) => true,
                Some(Queued::Bytes(_)) => c.credit > 0,
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
        let Some(next) = c.q.front_mut() else {
            let (code, reason) = c.close_after.take().unwrap_or((CLOSE_NORMAL, String::new()));
            self.chans.remove(&id);
            let mut p = code.to_le_bytes().to_vec();
            p.extend_from_slice(reason.as_bytes());
            return Some(Frame { chan: id, kind: kind::CLOSE, flags: 0, payload: p });
        };
        let (payload, more) = match next {
            Queued::Empty(count) => {
                *count -= 1;
                if *count == 0 {
                    c.q.pop_front();
                }
                (Vec::new(), false)
            }
            Queued::Bytes(msg) => {
                let n = (msg.len() - c.off).min(c.credit).min(slice);
                let more = c.off + n < msg.len();
                let payload = if c.off == 0 && !more { std::mem::take(msg) } else { msg[c.off..c.off + n].to_vec() };
                if more {
                    c.off += n;
                } else {
                    c.q.pop_front();
                    c.off = 0;
                }
                c.credit -= n;
                c.queued -= n;
                if c.echo {
                    // A stalled reader holds at most its receive window in echoed bytes.
                    Conn::credit(self, id, n);
                }
                (payload, more)
            }
        };
        Some(Frame { chan: id, kind: kind::DATA, flags: if more { FLAG_MORE } else { 0 }, payload })
    }

    fn queue_pong(&mut self, cookie: [u8; 8], now: Instant) -> bool {
        if self.pong.is_some() || self.last_pong.is_some_and(|last| now.saturating_duration_since(last) < PONG_INTERVAL) {
            return false;
        }
        self.pong = Some(cookie);
        true
    }
}

impl Conn {
    /// Starts the reader and writer threads over `io`, after the handshake. `initiator`: the
    /// side that dialled (it allocates odd channel numbers, the other even). A connection with a
    /// `handler` serves its peer: its threads' CPU counts as served (else as asked).
    pub fn start(io: Io, peer: Hello, initiator: bool, handler: Option<OpenHandler>, hooks: Hooks) -> Conn {
        Self::start_with_options(io, peer, initiator, handler, hooks, ConnOptions::default())
    }

    pub fn start_with_options(io: Io, peer: Hello, initiator: bool, handler: Option<OpenHandler>, hooks: Hooks, options: ConnOptions) -> Conn {
        let lookup = handler.map(|handler| Arc::new(move |name: &str| Some(ServiceOpen {
            limits: service_limits(name), handler: handler.clone(),
        })) as ServiceLookup);
        Self::start_registry(io, peer, initiator, lookup, hooks, options)
    }

    pub(crate) fn start_registry(io: Io, peer: Hello, initiator: bool, lookup: Option<ServiceLookup>, hooks: Hooks, options: ConnOptions) -> Conn {
        let serves = lookup.is_some();
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
            lookup,
            options,
            hooks,
            ping_limit_ns: AtomicU64::new(0),
            ping_out_ns: AtomicU64::new(0),
        });
        {
            let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
            live.retain(|c| c.strong_count() > 0);
            live.push(Arc::downgrade(&shared));
        }
        let conn = Conn(shared);
        let (c1, c2) = (conn.clone(), conn.clone());
        let writer = io.writer;
        let _ = std::thread::Builder::new().name("makepad-remote-rx".into()).spawn(move || {
            (c1.0.hooks.enter)();
            c1.read_loop(reader, serves);
            (c1.0.hooks.exit)();
        });
        let _ = std::thread::Builder::new().name("makepad-remote-tx".into()).spawn(move || {
            (c2.0.hooks.enter)();
            c2.write_loop(writer, serves);
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
        self.ping_out();
    }

    /// Ends the connection as lost once a ping has gone `limit` with nothing back from the peer
    /// (any frame answers it), and pings at least twice within `limit` while idle; zero (the
    /// default) never does. A peer's reader answers pings whatever its services do, so only a
    /// process that holds its end without serving it stays silent: a run that inherited a
    /// master's connection kept it open past that master's exec into its next build, and a request
    /// on it waited for good, never told the link had gone. Its open requests fail with
    /// [`crate::link::Error::LinkLost`], and a client dials again for the next.
    pub fn set_ping_limit(&self, limit: Duration) {
        self.0.ping_limit_ns.store(limit.as_nanos().min(u64::MAX as u128) as u64, Relaxed);
        self.0.work.notify_one();
    }

    /// A ping went: the oldest one out is the one the limit counts from.
    fn ping_out(&self) {
        let _ = self.0.ping_out_ns.compare_exchange(0, mono_ns().max(1), Relaxed, Relaxed);
    }

    /// Why the link is lost, once a ping has gone unanswered past the limit.
    fn ping_lost(&self) -> Option<String> {
        let (limit, out) = (self.0.ping_limit_ns.load(Relaxed), self.0.ping_out_ns.load(Relaxed));
        (limit > 0 && out > 0 && mono_ns().saturating_sub(out) >= limit).then(|| format!("the peer answered no ping for {:.1} s", Duration::from_nanos(limit).as_secs_f32()))
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

    /// Says goodbye and closes. The goodbye goes ahead of queued channel data and the peer ends the
    /// link on reading it, so data still queued may never reach it: to end after its data, close
    /// the channel ([`ChannelTx::close`]).
    pub fn close(&self, reason: &str) {
        self.0.closing.store(true, Relaxed);
        let mut st = self.0.st.lock().unwrap();
        st.push_ctrl(0, kind::DATA, hello::encode_goodbye(reason));
        st.draining = true;
        drop(st);
        self.0.work.notify_one();
    }

    fn terminate(&self, reason: &str) {
        self.terminate_with(CLOSE_LINK_LOST, reason);
    }

    fn terminate_with(&self, code: u16, reason: &str) {
        if self.0.closed.swap(true, Relaxed) {
            return;
        }
        *self.0.reason.lock().unwrap() = reason.to_string();
        let chans = std::mem::take(&mut self.0.st.lock().unwrap().chans);
        for (_, c) in chans {
            let _ = c.tx.send((Event::Closed(code, reason.to_string()), 0));
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
        self.open_bounded(service, args, (self.0.options.open_max_message)(service), wait)
    }

    /// Opens a channel with a local ceiling on incoming DATA messages, including service
    /// headers. A limit supplied by the service in OK can only lower this ceiling.
    pub fn open_bounded(&self, service: &str, args: &str, max_message: usize, wait: Duration) -> Result<Channel, (u16, String)> {
        // OPEN is one control frame (only DATA is chunked). Refuse before
        // queueing so an oversized call cannot tear down the whole carrier.
        let bytes = service.len().saturating_add(1).saturating_add(args.len());
        if bytes > MAX_PAYLOAD {
            return Err((CLOSE_REFUSED, format!("the encoded call is {bytes} bytes; one call carries {MAX_PAYLOAD}. Send a smaller first part, then append the rest with the service's edit tool.")));
        }
        let (tx, rx) = channel();
        let (id, generation) = {
            let mut st = self.0.st.lock().unwrap();
            if !self.is_usable() {
                return Err((CLOSE_LINK_LOST, self.close_reason()));
            }
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
            let generation = st.insert(id, service, max_message, tx, false, None);
            st.push_ctrl_flags(id, kind::OPEN, FLAG_MESSAGE_LIMIT, format!("{service}\n{args}").into_bytes());
            (id, generation)
        };
        self.0.work.notify_one();
        let ch = Channel { tx: ChannelTx { conn: self.clone(), id, generation }, rx, close_on_drop: true };
        match ch.rx.recv_timeout(wait) {
            Ok((Event::Opened(_), _)) => Ok(ch),
            Ok((Event::Closed(code, reason), _)) => Err((code, reason)),
            Ok((Event::Data(_), _)) => Err((CLOSE_PROTOCOL, "data before OK".into())),
            Err(_) => Err((CLOSE_LINK_LOST, "no answer to OPEN".into())),
        }
    }

    // ---- reader

    fn read_loop(&self, mut rd: FrameReader, serves: bool) {
        loop {
            crate::served::tick(serves);
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
            // anything from the peer answers the pings out
            self.0.ping_out_ns.store(0, Relaxed);
            if let Err(msg) = self.on_frame(f) {
                self.protocol_error(msg);
                return;
            }
            if self.is_closed() {
                return;
            }
        }
    }

    fn protocol_error(&self, reason: String) {
        self.0.closing.store(true, Relaxed);
        let mut st = self.0.st.lock().unwrap();
        st.protocol_error = Some(reason.clone());
        self.0.work.notify_one();
        // A peer may also have stopped reading. Give the writer a chance to send CLOSE, then
        // shut down the carrier to wake a blocked write and release the channels either way.
        let (st, _) = self.0.space.wait_timeout_while(st, Duration::from_secs(1), |_| !self.is_closed()).unwrap();
        drop(st);
        self.terminate_with(CLOSE_PROTOCOL, &reason);
    }

    fn on_frame(&self, f: Frame) -> Result<(), String> {
        match f.kind {
            kind::PING if self.0.options.strict => {
                if let (0, Ok(cookie)) = (f.chan, f.payload.as_slice().try_into()) {
                    if self.0.st.lock().unwrap().queue_pong(cookie, Instant::now()) {
                        self.0.work.notify_one();
                    }
                }
            }
            kind::PING => self.ctrl(0, kind::PONG, f.payload),
            kind::PONG => {
                if let Ok(b) = <[u8; 8]>::try_from(f.payload.as_slice()) {
                    self.0.rtt_ns.store(mono_ns().saturating_sub(u64::from_le_bytes(b)), Relaxed);
                }
            }
            kind::DATA if f.chan == 0 => self.on_control(&f.payload),
            kind::OPEN => self.on_open(f)?,
            kind::OK => {
                let mut st = self.0.st.lock().unwrap();
                if let Some(c) = st.chans.get_mut(&f.chan) {
                    if !c.awaiting_ok {
                        return if self.0.options.strict { Err(format!("unexpected OK on channel {}", f.chan)) } else { Ok(()) };
                    }
                    c.awaiting_ok = false;
                    let mut caps = f.payload.as_slice();
                    if f.flags & FLAG_MESSAGE_LIMIT != 0 {
                        let (limit, rest) = caps.split_at_checked(8).ok_or_else(|| format!("short OK limit on channel {}", f.chan))?;
                        let limit = usize::try_from(u64::from_le_bytes(limit.try_into().unwrap())).unwrap_or(usize::MAX);
                        c.max_message = c.max_message.min(limit);
                        caps = rest;
                    }
                    let _ = c.tx.send((Event::Opened(String::from_utf8_lossy(caps).into_owned()), 0));
                }
            }
            kind::DATA => {
                let mut st = self.0.st.lock().unwrap();
                let mut credit = None;
                if let Some(c) = st.chans.get_mut(&f.chan) {
                    if c.awaiting_ok {
                        return Err(format!("DATA before OK on channel {}", f.chan));
                    }
                    let n = f.payload.len();
                    c.recv_credit = c.recv_credit.checked_sub(n).ok_or_else(|| {
                        format!("DATA on channel {} uses {n} bytes with {} bytes of credit left", f.chan, c.recv_credit)
                    })?;
                    let size = c.reasm.len().saturating_add(n);
                    if size > c.max_message {
                        let reason = format!("channel {} message is {size} bytes; the limit is {}", f.chan, c.max_message);
                        let _ = c.tx.send((Event::Closed(CLOSE_PROTOCOL, reason.clone()), 0));
                        st.chans.remove(&f.chan);
                        st.push_close(f.chan, CLOSE_PROTOCOL, &reason);
                        drop(st);
                        self.0.space.notify_all();
                        self.0.work.notify_one();
                        return Ok(());
                    }
                    let more = f.flags & FLAG_MORE != 0;
                    let fragmented = more || c.partial;
                    let data = if fragmented {
                        c.reasm.extend_from_slice(&f.payload);
                        c.partial = more;
                        (!more).then(|| std::mem::take(&mut c.reasm))
                    } else { Some(f.payload) };
                    if more {
                        credit = Some(c.advance_partial());
                    }
                    if let Some(data) = data {
                        if c.echo {
                            c.enqueue(data);
                        } else {
                            let owed = data.len() - std::mem::take(&mut c.advanced);
                            c.unread += data.len();
                            let _ = c.tx.send((Event::Data(data), owed));
                        }
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
                        c.credit = c.credit.saturating_add(u32::from_le_bytes(b) as usize);
                        drop(st);
                        self.0.work.notify_one();
                    }
                }
            }
            kind::CLOSE => {
                let code = f.payload.get(..2).map_or(CLOSE_NORMAL, |b| u16::from_le_bytes([b[0], b[1]]));
                let reason = f.payload.get(2..).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
                if f.chan == 0 {
                    self.terminate_with(code, &reason);
                    return Ok(());
                }
                let mut st = self.0.st.lock().unwrap();
                if let Some(c) = st.chans.remove(&f.chan) {
                    let _ = c.tx.send((Event::Closed(code, reason), 0));
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
        let text = String::from_utf8_lossy(&f.payload);
        let (service, args) = text.split_once('\n').unwrap_or((&text, ""));
        let found = self.0.lookup.as_ref().and_then(|lookup| lookup(service));
        let mut st = self.0.st.lock().unwrap();
        if f.chan == 0 || st.chans.contains_key(&f.chan) {
            return Err(format!("OPEN of channel {} which is already in use", f.chan));
        }
        let Some(svc) = found else {
            let reason = if service.len() <= 255 { format!("unsupported service {service}") } else { "unsupported service".into() };
            st.push_close(f.chan, CLOSE_UNSUPPORTED, &reason);
            self.0.work.notify_one();
            return Ok(());
        };
        let admission = if self.0.options.strict {
            let Some(slot) = st.admit(service, svc.limits.max_channels) else {
                st.push_close(f.chan, CLOSE_REFUSED, &format!("all {} channels of {service} are in use", svc.limits.max_channels));
                self.0.work.notify_one();
                return Ok(());
            };
            Some(slot)
        } else { None };
        let (tx, rx) = channel();
        // Only a strict link bounds a message (see `ConnOptions::default`).
        let max_message = if self.0.options.strict { svc.limits.max_message } else { usize::MAX };
        let generation = st.insert(f.chan, service, max_message, tx, true, admission);
        drop(st);
        let channel = Channel { tx: ChannelTx { conn: self.clone(), id: f.chan, generation }, rx, close_on_drop: true };
        let open = Open { service: service.to_string(), args: args.to_string(), channel };
        let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (svc.handler)(self, open)))
            .unwrap_or_else(|_| Err((CLOSE_REFUSED, "the service failed to open".into())));
        let mut st = self.0.st.lock().unwrap();
        match answer {
            Ok(caps) => {
                if let Some(c) = st.chans.get_mut(&f.chan) {
                    c.hold = false;
                }
                if f.flags & FLAG_MESSAGE_LIMIT != 0 && caps.len() <= MAX_PAYLOAD - 8 {
                    let mut payload = (max_message as u64).to_le_bytes().to_vec();
                    payload.extend_from_slice(caps.as_bytes());
                    st.push_ctrl_flags(f.chan, kind::OK, FLAG_MESSAGE_LIMIT, payload);
                } else {
                    st.push_ctrl(f.chan, kind::OK, caps.into_bytes());
                }
            }
            Err((code, reason)) => {
                st.chans.remove(&f.chan);
                st.push_close(f.chan, code, &reason);
            }
        }
        drop(st);
        self.0.work.notify_one();
        Ok(())
    }

    // ---- writer

    fn write_loop(&self, w: Box<dyn Write + Send>, serves: bool) {
        let mut w = BufWriter::with_capacity(64 << 10, w);
        let mut flushed = true;
        let mut idle_since = std::time::Instant::now();
        loop {
            crate::served::tick(serves);
            let work = {
                let mut st = self.0.st.lock().unwrap();
                loop {
                    if self.is_closed() {
                        return;
                    }
                    if let Some(reason) = st.protocol_error.take() {
                        break Work::ProtocolError(reason);
                    }
                    if let Some(f) = st.pick(self.0.options.frame_slice) {
                        break Work::Frame(f);
                    }
                    if !flushed {
                        break Work::Flush;
                    }
                    if st.draining {
                        break Work::Exit;
                    }
                    if let Some(why) = self.ping_lost() {
                        break Work::Lost(why);
                    }
                    // a ping limit pings at least twice within it
                    let limit = self.0.ping_limit_ns.load(Relaxed);
                    let every = if limit == 0 { KEEPALIVE } else { KEEPALIVE.min(Duration::from_nanos(limit / 2)) };
                    let (g, to) = self.0.work.wait_timeout(st, every).unwrap();
                    st = g;
                    if to.timed_out() && idle_since.elapsed() >= every {
                        st.push_ctrl(0, kind::PING, mono_ns().to_le_bytes().to_vec());
                        self.ping_out();
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
                Work::Lost(why) => {
                    self.terminate(&why);
                    return;
                }
                Work::ProtocolError(reason) => {
                    let mut payload = CLOSE_PROTOCOL.to_le_bytes().to_vec();
                    payload.extend_from_slice(reason.as_bytes());
                    let _ = write_frame(&mut w, 0, kind::CLOSE, 0, &payload);
                    let _ = w.flush();
                    self.terminate_with(CLOSE_PROTOCOL, &reason);
                    return;
                }
            };
            if let Err(e) = r {
                self.terminate(&format!("write failed: {e}"));
                return;
            }
        }
    }

    /// Counts `n` consumed bytes of channel `id`; hands the sender more credit past a quarter window.
    fn credit(st: &mut State, id: u16, n: usize) {
        let Some(c) = st.chans.get_mut(&id) else { return };
        c.ungranted += n;
        if c.ungranted >= GRANT_AT {
            let g = std::mem::take(&mut c.ungranted) as u32;
            st.ctrl.push_back(Control::Grant { id, generation: c.generation, bytes: g });
        }
    }
}

/// The sending half of a channel; cloneable, usable from any thread.
#[derive(Clone)]
pub struct ChannelTx {
    conn: Conn,
    id: u16,
    generation: u64,
}

impl ChannelTx {
    fn current<'a>(&self, st: &'a mut State) -> Option<&'a mut Chan> {
        st.chans.get_mut(&self.id).filter(|c| c.generation == self.generation)
    }

    pub fn id(&self) -> u16 {
        self.id
    }

    pub fn conn(&self) -> &Conn {
        &self.conn
    }

    /// Queues `msg` unless the queue is full (never blocks).
    pub fn try_send(&self, msg: Vec<u8>) -> Result<(), SendError> {
        let mut st = self.conn.0.st.lock().unwrap();
        let Some(c) = self.current(&mut st) else { return Err(SendError::Closed) };
        if c.close_after.is_some() {
            return Err(SendError::Closed);
        }
        if c.queued > 0 && c.queued + msg.len() > QUEUE_CAP {
            return Err(SendError::Full(msg));
        }
        c.enqueue(msg);
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
            let Some(c) = self.current(&mut st) else { return Err(broken()) };
            if c.close_after.is_some() {
                return Err(broken());
            }
            if c.queued == 0 || c.queued + msg.len() <= QUEUE_CAP {
                c.enqueue(msg);
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
        let now = match self.current(&mut st) {
            // Nothing left to send (and answered, if the peer opened it): the CLOSE goes out in
            // order with the control frames, so an OPEN queued after it can't overtake it and
            // find the peer still counting this channel against the service's cap.
            Some(c) if !c.hold && c.q.is_empty() && c.close_after.is_none() => true,
            Some(c) => {
                if c.close_after.is_none() {
                    c.close_after = Some((code, reason.to_string()));
                }
                false
            }
            None => false,
        };
        if now {
            st.chans.remove(&self.id);
            st.push_close(self.id, code, reason);
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
    /// (event, bytes to return as credit when consumed)
    rx: Receiver<(Event, usize)>,
    close_on_drop: bool,
}

impl Channel {
    /// Transfer a short-message echo service to the reader. Its writer returns the credit as it
    /// sends replies, so the reader never waits for space or a worker.
    pub(crate) fn echo_on_reader(mut self) {
        if let Some(c) = self.tx.current(&mut self.conn().0.st.lock().unwrap()) {
            c.echo = true;
        }
        self.close_on_drop = false;
    }

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

    fn got(&self, (e, owed): (Event, usize)) -> Event {
        if let Event::Data(d) = &e {
            let mut st = self.conn().0.st.lock().unwrap();
            if let Some(c) = self.tx.current(&mut st) {
                c.unread -= d.len();
                let advance = c.advance_partial();
                Conn::credit(&mut st, self.id(), owed + advance);
                drop(st);
                self.conn().0.work.notify_one();
            }
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
        if self.close_on_drop {
            self.tx.close(CLOSE_NORMAL, "");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_pongs_have_one_pending_reply_and_no_burst() {
        let mut st = State::default();
        let now = Instant::now();
        for i in 0..20 {
            assert_eq!(st.queue_pong([i; 8], now), i == 0);
        }
        assert_eq!(st.pick(STRICT_SLICE).unwrap().payload, [0; 8]);
        let sent = st.last_pong.unwrap();
        for ms in 0..250 {
            assert!(!st.queue_pong([1; 8], sent + Duration::from_millis(ms)));
        }
        assert!(st.pick(STRICT_SLICE).is_none(), "excess PINGs left work for the writer");
        assert!(st.queue_pong([2; 8], sent + PONG_INTERVAL));
        assert_eq!(st.pick(STRICT_SLICE).unwrap().payload, [2; 8]);
    }

    #[cfg(unix)]
    #[test]
    fn fragmented_records_wait_for_the_consumer() {
        let ep = crate::Endpoint::new("test");
        let (tx, rx) = channel();
        ep.register("frames", 1, Arc::new(move |_, open| {
            tx.send(open.channel).unwrap();
            Ok(())
        }));
        let (client, server) = crate::tests::pair_served(&ep);
        let sender = client.open("frames", "").unwrap();
        let receiver = rx.recv().unwrap();
        let size = INITIAL_CREDIT * 2;
        sender.send(vec![1; size]).unwrap();
        // Synchronize on the reader completing the record, without consuming it.
        loop {
            let mut st = server.0.st.lock().unwrap();
            let c = receiver.tx.current(&mut st).unwrap();
            if c.unread >= size {
                assert_eq!(c.unread, size);
                assert_eq!(c.recv_credit + c.ungranted, 0);
                assert!(!st.ctrl.iter().any(|ctl| matches!(ctl, Control::Grant { id, .. } if *id == receiver.id())));
                break;
            }
            drop(st);
            std::thread::yield_now();
        }
        sender.try_send(vec![2; size]).unwrap();
        sender.try_send(vec![3; size]).unwrap();
        let Err(SendError::Full(fourth)) = sender.try_send(vec![4; size]) else {
            panic!("unread fragmented records must stop the writer");
        };
        let drain = std::thread::spawn(move || {
            for value in 1..=4 {
                let Some(Event::Data(bytes)) = receiver.recv() else { panic!("stream ended early") };
                assert_eq!(bytes, vec![value; size], "record {value} arrived whole and in order");
            }
        });
        sender.send(fourth).unwrap();
        drain.join().unwrap();
        client.close("done");
        server.close("done");
    }

    #[test]
    fn writer_rechecks_priority_between_slices() {
        let mut st = State::default();
        st.insert(1, "stream", INITIAL_CREDIT, channel().0, false, None);
        st.insert(3, "cmd", INITIAL_CREDIT, channel().0, false, None);
        let slice = ConnOptions::strict().frame_slice;
        st.chans.get_mut(&1).unwrap().enqueue(vec![0; 2 * slice]);
        let first = st.pick(slice).unwrap();
        assert_eq!((first.chan, first.payload.len(), first.flags), (1, 16 << 10, FLAG_MORE));
        st.chans.get_mut(&3).unwrap().enqueue(b"cmd".to_vec());
        let urgent = st.pick(slice).unwrap();
        assert_eq!((urgent.chan, urgent.payload.as_slice()), (3, &b"cmd"[..]));
        let last = st.pick(slice).unwrap();
        assert_eq!((last.chan, last.payload.len(), last.flags), (1, 16 << 10, 0));
    }
}
