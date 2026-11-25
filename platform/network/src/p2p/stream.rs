//! A reliable, ordered byte stream on one punched UDP socket. Construct it once
//! punch reports an open path. The link worker supplies monotonic milliseconds,
//! drains `poll_transmit` after input, reads and writes, and arms `next_timeout`.
//! All stream state belongs to that worker.
//!
//! Before key installation, the stream accepts untrusted TLS bytes only on the original path.
//! After `set_keys`, it tags every send and verifies every incoming tag. It still accepts
//! unsigned DATA, ACK (including window updates) and PING from the original address until
//! the first verified peer datagram or a cumulative ACK covering all bytes written at key
//! installation, whichever comes first. At that boundary it discards buffered out-of-order
//! segments for tagged retransmission and requires a valid tag on every subsequent datagram.
//! Only verified packets enter the authenticated replay window or authorize a path change.
//! A forged unsigned ACK during cutover can close the gap early or acknowledge bytes sent
//! but never received. A zero-window probe recovers missed credit, and TLS detects broken
//! records or handshake transcripts. Missing bytes can stall or fail the handshake without
//! allowing unauthenticated content through TLS.
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;

mod send;
mod wire;
use send::{Sender, MAX_RTO, SLOTS};
use wire::{expand, Packet, ACCEPT, ACK, CHALLENGE, DATA, FIN, MAX_PN, OPEN, PING, RESET, RESPONSE};

const ACK_DELAY: u64 = 25;
const KEEPALIVE: u64 = 15_000;
const IDLE: u64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Client,
    Server,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    Finished,
    TimedOut,
    Reset,
    PacketNumbersExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Opening,
    Open,
    Closing,
    Closed(CloseReason),
}

#[derive(Default)]
#[cfg_attr(test, derive(Clone, PartialEq))]
struct Replay {
    high: u64,
    seen: u128,
}

impl Replay {
    /// Some(true) for a new high water mark, Some(false) for unseen reordering.
    fn admit(&mut self, pn: u64) -> Option<bool> {
        if pn > self.high {
            let shift = pn - self.high;
            self.seen = if shift < 128 { self.seen << shift } else { 0 } | 1;
            self.high = pn;
            return Some(true);
        }
        let age = self.high - pn;
        if age >= 128 || self.seen & (1 << age) != 0 {
            return None;
        }
        self.seen |= 1 << age;
        Some(false)
    }
}

#[cfg_attr(test, derive(Clone, PartialEq))]
struct Candidate {
    addr: SocketAddr,
    token: [u8; 8],
    retry_at: u64,
    expires: u64,
}

#[cfg_attr(test, derive(Clone, PartialEq))]
struct Receiver {
    next: u64,
    // Each unread or reordered segment occupies its own reserved slot. Reading
    // a whole segment advances the right edge, even across a sequence wrap.
    edge: u64,
    reordered: BTreeMap<u64, Vec<u8>>,
    ready: VecDeque<Vec<u8>>,
    offset: usize,
    finished: bool,
    echo: u64,
    ack_at: Option<u64>,
    unacked: u8,
}

impl Receiver {
    fn credit(&self) -> u16 {
        (self.edge - self.next) as u16
    }

    fn ack_by(&mut self, at: u64) {
        self.ack_at = Some(self.ack_at.map_or(at, |old| old.min(at)));
    }

    fn segment(&mut self, packet: &Packet<'_>, now: u64) {
        self.echo = packet.number;
        let seq = expand(packet.seq, self.next);
        if self.finished || seq != Some(self.next) {
            if let Some(seq) = seq {
                if !self.finished && seq > self.next && seq < self.edge {
                    self.reordered.entry(seq).or_insert_with(|| packet.body.to_vec());
                }
            }
            self.ack_by(now);
            return;
        }
        if self.next == self.edge {
            self.ack_by(now);
            return;
        }
        let filled_gap = !self.reordered.is_empty();
        let mut bytes = packet.body.to_vec();
        loop {
            self.next += 1;
            if bytes.is_empty() {
                self.finished = true;
                self.reordered.clear();
                break;
            }
            self.ready.push_back(bytes);
            let Some(next) = self.reordered.remove(&self.next) else { break };
            bytes = next;
        }
        self.unacked = self.unacked.saturating_add(1);
        self.ack_by(if self.unacked >= 2 || self.finished || filled_gap { now } else { now + ACK_DELAY });
    }

    fn read(&mut self, out: &mut [u8], now: u64) -> usize {
        let was_full = self.next == self.edge;
        let old_edge = self.edge;
        let mut used = 0;
        while used < out.len() {
            let Some(front) = self.ready.front() else { break };
            let n = (out.len() - used).min(front.len() - self.offset);
            out[used..used + n].copy_from_slice(&front[self.offset..self.offset + n]);
            used += n;
            self.offset += n;
            if self.offset == front.len() {
                self.offset = 0;
                self.ready.pop_front();
                self.edge += 1;
            }
        }
        if self.edge != old_edge {
            self.ack_by(if was_full { now } else { now + ACK_DELAY });
        }
        used
    }
}

#[cfg_attr(test, derive(Clone, PartialEq))]
pub struct Stream {
    role: Role,
    id: [u8; 8],
    peer: SocketAddr,
    state: State,
    now: u64,
    deadline: u64,
    last_sent: u64,
    probe_at: u64,
    open_at: u64,
    open_interval: u64,
    accept_pending: bool,
    next_packet: u64,
    keys: Option<([u8; 32], [u8; 32])>,
    unsigned_until_ack: Option<u64>, // write offset at key installation, while cutover is pending
    unverified_replay: Replay,
    verified_replay: Replay,
    candidate: Option<Candidate>,
    response: Option<(SocketAddr, [u8; 8])>,
    tx: Sender,
    rx: Receiver,
    linger_until: Option<u64>,
}

impl Stream {
    /// The caller has opened the path and assigned the roles for this connection ID.
    pub fn new(role: Role, id: [u8; 8], peer: SocketAddr, now_ms: u64) -> Self {
        Self {
            role,
            id,
            peer,
            state: State::Opening,
            now: now_ms,
            deadline: now_ms + IDLE,
            last_sent: now_ms,
            probe_at: now_ms,
            open_at: now_ms,
            open_interval: 1000,
            accept_pending: false,
            next_packet: 1,
            keys: None,
            unsigned_until_ack: None,
            unverified_replay: Replay::default(),
            verified_replay: Replay::default(),
            candidate: None,
            response: None,
            tx: Sender::new(),
            rx: Receiver {
                next: 0,
                edge: u64::from(SLOTS),
                reordered: BTreeMap::new(),
                ready: VecDeque::new(),
                offset: 0,
                finished: false,
                echo: 0,
                ack_at: None,
                unacked: 0,
            },
            linger_until: None,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// The validated destination. A change here reports a completed path move.
    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// The link worker must write all of this side's TLS handshake output into the
    /// stream before calling this, retrying partial writes until all bytes are accepted.
    /// Installs the directional keys once on an open stream and preserves packet numbering.
    pub fn set_keys(&mut self, send_key: [u8; 32], recv_key: [u8; 32]) -> bool {
        if self.state != State::Open || self.keys.is_some() {
            return false;
        }
        self.keys = Some((send_key, recv_key));
        self.unsigned_until_ack = Some(self.tx.written_bytes);
        self.check_cutover_ack();
        self.deadline = self.now + IDLE;
        true
    }

    fn gap_closed(&self) -> bool {
        self.keys.is_some() && self.unsigned_until_ack.is_none()
    }

    fn close_gap(&mut self) {
        if self.unsigned_until_ack.take().is_some() {
            // Buffered out-of-order segments have never been cumulatively ACKed.
            self.rx.reordered.clear();
        }
    }

    fn check_cutover_ack(&mut self) {
        if self.unsigned_until_ack.is_some_and(|offset| self.tx.acked_bytes >= offset) {
            self.close_gap();
        }
    }

    /// Returns the number of bytes accepted. An open stream with no room returns zero.
    pub fn write(&mut self, bytes: &[u8]) -> usize {
        if self.state == State::Open {
            self.tx.write(bytes)
        } else {
            0
        }
    }

    /// Reads contiguous bytes, including buffered bytes after a clean close.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        self.rx.read(out, self.now)
    }

    /// Drains accepted writes, exchanges ordered FINs, and acknowledges repeated
    /// FINs during the closing interval. An unauthenticated attempt ends on timeout.
    pub fn close(&mut self) {
        if self.keys.is_some() && self.state == State::Open {
            self.state = State::Closing;
            self.tx.finish();
        }
    }

    fn advance(&mut self, now: u64) -> bool {
        self.now = now;
        if self.linger_until.is_some_and(|t| now >= t) {
            self.state = State::Closed(CloseReason::Finished);
        } else if now >= self.deadline && !matches!(self.state, State::Closed(_)) {
            self.state = State::Closed(CloseReason::TimedOut);
        }
        !matches!(self.state, State::Closed(_))
    }

    fn path_control(&mut self, from: SocketAddr, p: &Packet<'_>, highest: bool) {
        if p.kind == CHALLENGE {
            self.response = Some((from, p.body.try_into().unwrap()));
        }
        if from == self.peer || !highest {
            return;
        }
        if let Some(path) = &self.candidate {
            if path.addr == from && path.expires > self.now {
                if p.kind == RESPONSE && crate::tls::constant_time_eq(p.body, &path.token) {
                    self.peer = from;
                    self.candidate = None;
                    self.deadline = self.now + IDLE;
                }
                return;
            }
        }
        // A keyed, unpredictable challenge unique to this candidate observation.
        // Its domain prefix cannot be the start of a stream datagram.
        let mut input = *b"path\0\0\0\0\0\0\0\0";
        input[4..].copy_from_slice(&p.number.to_be_bytes());
        let mac = crate::tunnel::hmac_sha256(&self.keys.as_ref().unwrap().0, &input);
        self.candidate = Some(Candidate {
            addr: from,
            token: mac[..8].try_into().unwrap(),
            retry_at: self.now,
            expires: self.now + IDLE,
        });
    }

    pub fn handle_datagram(&mut self, now_ms: u64, from: SocketAddr, bytes: &[u8]) {
        if matches!(self.state, State::Closed(_)) || (from != self.peer && self.keys.is_none()) {
            return;
        }
        let Some((p, verified)) = Packet::decode(bytes, &self.id, self.keys.as_ref().map(|k| &k.1)) else { return };
        // A server listens only for an OPEN. Ignore other input before it can
        // consume a number in the listener's replay window.
        if self.state == State::Opening && !matches!((self.role, p.kind), (Role::Server, OPEN) | (Role::Client, ACCEPT))
        {
            return;
        }
        let highest = if verified {
            self.verified_replay.admit(p.number)
        } else {
            if from != self.peer || self.gap_closed() || (self.keys.is_some() && !matches!(p.kind, DATA | ACK | PING)) {
                return;
            }
            self.unverified_replay.admit(p.number)
        };
        let Some(highest) = highest else { return };
        if !self.advance(now_ms) {
            return;
        }
        if verified {
            self.close_gap();
            if from != self.peer || matches!(p.kind, CHALLENGE | RESPONSE) {
                self.path_control(from, &p, highest);
                return;
            }
        }
        match p.kind {
            OPEN if self.role == Role::Server => {
                self.accept_pending = true;
                if self.state == State::Opening {
                    self.state = State::Open;
                }
            }
            ACCEPT if self.role == Role::Client && self.state == State::Opening => self.state = State::Open,
            OPEN | ACCEPT => return,
            RESET => {
                self.state = State::Closed(CloseReason::Reset);
                return;
            }
            DATA | FIN => {
                self.rx.segment(&p, now_ms);
                if self.rx.finished {
                    self.close();
                    if self.linger_until.is_some() && p.kind == FIN {
                        self.linger_until = Some(now_ms + 2 * MAX_RTO);
                    }
                }
            }
            PING => self.rx.ack_by(now_ms),
            _ => (),
        }
        self.tx.acknowledge(&p, now_ms);
        self.check_finished();
        if verified {
            self.deadline = now_ms + IDLE;
        } else {
            self.check_cutover_ack();
        }
    }

    fn check_finished(&mut self) {
        if self.rx.finished && self.tx.fin_acked && self.rx.ack_at.is_none() && self.linger_until.is_none() {
            self.linger_until = Some(self.now + 2 * MAX_RTO);
        }
    }

    pub fn poll_transmit(&mut self, now_ms: u64) -> Option<(SocketAddr, Vec<u8>)> {
        if !self.advance(now_ms) {
            return None;
        }
        if self.next_packet > MAX_PN {
            self.state = State::Closed(CloseReason::PacketNumbersExhausted);
            return None;
        }
        if self.candidate.as_ref().is_some_and(|p| p.expires <= now_ms) {
            self.candidate = None;
        }
        let probe_due = self.tx.blocked_on_credit() && self.probe_at <= now_ms;
        let probe_interval = self.tx.rto;
        let pn = self.next_packet;
        let mut to = self.peer;
        let mut seq = 0;
        let token: [u8; 8];
        let mut body: &[u8] = &[];
        let kind = if let Some((addr, echo)) = self.response.take() {
            to = addr;
            token = echo;
            body = &token;
            RESPONSE
        } else if let Some(path) = self.candidate.as_mut().filter(|p| p.retry_at <= now_ms) {
            to = path.addr;
            token = path.token;
            body = &token;
            path.retry_at = now_ms + self.tx.rto;
            CHALLENGE
        } else if self.accept_pending {
            self.accept_pending = false;
            ACCEPT
        } else if self.state == State::Opening {
            if self.role != Role::Client || self.open_at > now_ms {
                return None;
            }
            self.open_at = now_ms + self.open_interval;
            self.open_interval = (2 * self.open_interval).min(MAX_RTO);
            OPEN
        } else if let Some((s, payload)) = self.tx.transmit(now_ms, pn) {
            seq = s as u32;
            body = payload;
            if body.is_empty() {
                FIN
            } else {
                DATA
            }
        } else if self.rx.ack_at.is_some_and(|at| at <= now_ms) {
            ACK
        } else {
            if (!probe_due && now_ms < self.last_sent + KEEPALIVE) || self.linger_until.is_some() {
                return None;
            }
            self.probe_at = now_ms + probe_interval;
            PING
        };
        let packet = Packet {
            kind,
            number: pn,
            seq,
            ack: self.rx.next as u32,
            credit: self.rx.credit(),
            echo: self.rx.echo,
            body,
        };
        let bytes = packet.encode(&self.id, self.keys.as_ref().map(|k| &k.0));
        self.next_packet += 1;
        if !matches!(kind, CHALLENGE | RESPONSE) {
            self.last_sent = now_ms;
            self.rx.ack_at = None;
            self.rx.echo = 0;
            self.rx.unacked = 0;
            self.check_finished();
        }
        Some((to, bytes))
    }

    /// Absolute wake time. Recheck after every read, write and received datagram.
    pub fn next_timeout(&self) -> Option<u64> {
        if matches!(self.state, State::Closed(_)) {
            return None;
        }
        let mut at = self.linger_until.unwrap_or(self.deadline);
        if self.accept_pending || self.response.is_some() || self.next_packet > MAX_PN {
            at = self.now;
        }
        if let Some(path) = &self.candidate {
            at = at.min(path.retry_at.min(path.expires));
        }
        if self.state == State::Opening {
            if self.role == Role::Client {
                at = at.min(self.open_at);
            }
        } else {
            if let Some(ack) = self.rx.ack_at {
                at = at.min(ack);
            }
            if let Some(tx) = self.tx.wake(self.now) {
                at = at.min(tx);
            }
            if self.linger_until.is_none() {
                if self.tx.blocked_on_credit() {
                    at = at.min(self.probe_at);
                }
                at = at.min(self.last_sent + KEEPALIVE);
            }
        }
        Some(at)
    }
}

#[cfg(test)]
mod tests;
