//! Host and client endpoints.
//!
//! Both are **pumped**, not threaded: `pump(now)` drains sockets, enforces
//! deadlines and flushes writes, then returns events. Nothing blocks, so a
//! whole session — host plus N clients — runs deterministically inside one
//! test process against a virtual clock. Threads can wrap this later; the
//! audit's worst finding (a blocking connect on a worker thread stalling for
//! minutes and hanging shutdown) cannot occur in a design that never blocks.
//!
//! The host **only accepts** TCP connections and never initiates them, so a
//! flood of forged discovery beacons has nothing to drive it into connecting.

use crate::auth::LobbyKey;
use crate::protocol::*;
use makepad_micro_serde::*;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::time::Duration;

/// Hard ceiling on simultaneous players. Every per-peer structure is bounded by
/// this; the XR stack had no cap at all.
pub const MAX_PLAYERS: usize = 16;
/// Sockets accepted but not yet authenticated. Bounded so a connect flood
/// cannot exhaust file descriptors.
pub const MAX_PENDING_CONNECTIONS: usize = 8;
/// A connection that has not completed `Join` within this many seconds is
/// dropped — without it, half-open sockets accumulate forever.
pub const HANDSHAKE_DEADLINE: f64 = 3.0;
/// Silence after which a player is considered gone.
pub const PEER_TIMEOUT: f64 = 5.0;

const TCP_READ_BUDGET: usize = 256 * 1024;
const TCP_WRITE_BUDGET: usize = 256 * 1024;
/// UDP receive is budgeted exactly like TCP. The XR version looped until
/// `WouldBlock`, so a line-rate flood livelocked the worker (audit P0-5).
const UDP_DATAGRAM_BUDGET: usize = 256;
/// Backpressure: past this, `State` traffic is dropped oldest-first and
/// `Control` traffic disconnects the peer instead of growing without bound.
const WRITE_BUF_HIGH_WATER: usize = 1024 * 1024;
/// Bound on queued events handed to the application per pump cycle.
const MAX_EVENTS_PER_PUMP: usize = 4096;

const READ_CHUNK: usize = 16 * 1024;

fn bind_udp(addr: SocketAddr) -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind(addr)?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

/// A framed, authenticated TCP stream with budgeted IO and class-based
/// backpressure.
struct Connection {
    stream: TcpStream,
    addr: SocketAddr,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    /// `None` until an authenticated `Join` arrives.
    player: Option<PlayerId>,
    accepted_at: f64,
    overflowed: bool,
}

impl Connection {
    fn new(stream: TcpStream, addr: SocketAddr, now: f64) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        let _ = stream.set_nodelay(true);
        Ok(Self {
            stream,
            addr,
            read_buf: Vec::new(),
            write_buf: Vec::new(),
            player: None,
            accepted_at: now,
            overflowed: false,
        })
    }

    fn queue(&mut self, sender: u64, payload: &[u8], key: &LobbyKey, class: PacketClass) {
        if self.write_buf.len() > WRITE_BUF_HIGH_WATER {
            match class {
                // Superseded next tick: dropping is correct, disconnecting is not.
                PacketClass::State => return,
                // Reliable traffic cannot be silently dropped, so the peer goes.
                PacketClass::Control => {
                    self.overflowed = true;
                    return;
                }
            }
        }
        if let Some(frame) = FrameCodec::encode(sender, payload, key) {
            self.write_buf.extend_from_slice(&frame);
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut written_total = 0usize;
        while !self.write_buf.is_empty() {
            match self.stream.write(&self.write_buf) {
                Ok(0) => {
                    return Err(io::Error::new(io::ErrorKind::WriteZero, "write zero"));
                }
                Ok(written) => {
                    consume_prefix(&mut self.write_buf, written);
                    written_total += written;
                    if written_total >= TCP_WRITE_BUDGET {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn read_frames(&mut self, key: &LobbyKey) -> io::Result<Vec<(u64, Vec<u8>)>> {
        let mut chunk = [0u8; READ_CHUNK];
        let mut read_total = 0usize;
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed")),
                Ok(len) => {
                    self.read_buf.extend_from_slice(&chunk[..len]);
                    read_total += len;
                    if read_total >= TCP_READ_BUDGET {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        FrameCodec::drain(&mut self.read_buf, key)
    }
}

/// What the host application observes.
#[derive(Clone, Debug, PartialEq)]
pub enum HostEvent {
    Joined {
        player: PlayerId,
        name: String,
    },
    Left {
        player: PlayerId,
        reason: LeaveReason,
    },
    Input {
        player: PlayerId,
        frame: InputFrame,
    },
    Intent {
        player: PlayerId,
        intent: Intent,
    },
}

struct PlayerSlot {
    name: String,
    /// Pinned on join from the TCP peer; a datagram whose source does not match
    /// is discarded. The XR stack instead *overwrote* the stored address from
    /// whatever source arrived, which let one packet hijack a peer's traffic.
    ip: IpAddr,
    udp_addr: Option<SocketAddr>,
    last_seen: f64,
    /// Highest input tick applied. Reset on rejoin so a returning player whose
    /// tick counter restarts is not silently ignored forever.
    last_input_tick: Option<u64>,
}

pub struct HostConfig {
    pub host_id: HostId,
    pub name: String,
    pub lobby_key: LobbyKey,
    pub max_players: usize,
    pub bind_ip: IpAddr,
    pub tcp_port: u16,
    pub udp_port: u16,
}

impl HostConfig {
    pub fn new(name: &str, secret: &[u8]) -> Self {
        Self {
            host_id: HostId(0x4172_6361_6465_0001),
            name: name.to_string(),
            lobby_key: LobbyKey::new(secret),
            max_players: MAX_PLAYERS,
            bind_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            tcp_port: 0,
            udp_port: 0,
        }
    }
}

/// Authoritative endpoint. Owns simulation truth; clients only ever request.
pub struct Host {
    config: HostConfig,
    listener: TcpListener,
    udp: UdpSocket,
    tcp_addr: SocketAddr,
    udp_addr: SocketAddr,
    connections: Vec<Connection>,
    players: HashMap<PlayerId, PlayerSlot>,
    entity_seq: HashMap<u64, u32>,
    /// World state handed to joiners so they can reconstruct mid-session.
    pending_snapshot: Option<(u64, Vec<EntityState>)>,
    pub stats: NetStats,
}

/// Counters for tests and diagnostics — every rejection path is visible rather
/// than silent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetStats {
    pub datagrams_in: u64,
    pub datagrams_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub auth_failures: u64,
    pub source_mismatches: u64,
    pub stale_dropped: u64,
    pub rejected_full: u64,
    pub rejected_pending: u64,
    pub handshake_timeouts: u64,
    pub malformed: u64,
}

impl Host {
    pub fn bind(config: HostConfig) -> io::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::new(config.bind_ip, config.tcp_port))?;
        listener.set_nonblocking(true)?;
        let udp = bind_udp(SocketAddr::new(config.bind_ip, config.udp_port))?;
        let tcp_addr = listener.local_addr()?;
        let udp_addr = udp.local_addr()?;
        Ok(Self {
            config,
            listener,
            udp,
            tcp_addr,
            udp_addr,
            connections: Vec::new(),
            players: HashMap::new(),
            entity_seq: HashMap::new(),
            pending_snapshot: None,
            stats: NetStats::default(),
        })
    }

    pub fn tcp_addr(&self) -> SocketAddr {
        self.tcp_addr
    }
    pub fn udp_addr(&self) -> SocketAddr {
        self.udp_addr
    }
    pub fn player_count(&self) -> usize {
        self.players.len()
    }
    pub fn player_name(&self, player: PlayerId) -> Option<&str> {
        self.players.get(&player).map(|slot| slot.name.as_str())
    }
    pub fn players(&self) -> impl Iterator<Item = PlayerId> + '_ {
        self.players.keys().copied()
    }
    pub fn pending_connections(&self) -> usize {
        self.connections.iter().filter(|c| c.player.is_none()).count()
    }
    pub fn announce(&self) -> Announce {
        Announce {
            protocol: PROTOCOL_VERSION,
            host_id: self.config.host_id,
            name: self.config.name.clone(),
            players: self.players.len() as u16,
            max_players: self.config.max_players as u16,
            tcp_port: self.tcp_addr.port(),
            udp_port: self.udp_addr.port(),
        }
    }

    fn send_to(&mut self, index: usize, msg: &HostToClient, class: PacketClass) {
        let payload = msg.serialize_bin();
        let id = self.config.host_id.0;
        let key = self.config.lobby_key.clone();
        self.connections[index].queue(id, &payload, &key, class);
    }

    /// Reliable, ordered broadcast (spawns, removals, scores).
    pub fn broadcast_event(&mut self, tick: u64, event: GameEvent) {
        let msg = HostToClient::Event { tick, event };
        for i in 0..self.connections.len() {
            if self.connections[i].player.is_some() {
                self.send_to(i, &msg, PacketClass::Control);
            }
        }
    }

    /// Unreliable state replication. Each entity carries its own sequence, so a
    /// reordered or lost datagram only affects the entities inside it.
    pub fn broadcast_state(&mut self, tick: u64, entities: &[EntityState]) {
        if entities.is_empty() {
            return;
        }
        let stamped: Vec<EntityState> = entities
            .iter()
            .map(|e| {
                let seq = self.entity_seq.entry(e.id).or_insert(0);
                *seq = seq.wrapping_add(1);
                EntityState { seq: *seq, ..*e }
            })
            .collect();

        let targets: Vec<SocketAddr> = self
            .players
            .values()
            .filter_map(|slot| slot.udp_addr)
            .collect();
        if targets.is_empty() {
            return;
        }

        for batch in batch_entities(tick, &stamped) {
            let datagram = Envelope::seal(
                self.config.host_id.0,
                &batch.serialize_bin(),
                &self.config.lobby_key,
            );
            for addr in &targets {
                if self.udp.send_to(&datagram, addr).is_ok() {
                    self.stats.datagrams_out += 1;
                    self.stats.bytes_out += datagram.len() as u64;
                }
            }
        }
    }

    pub fn pump(&mut self, now: f64) -> Vec<HostEvent> {
        let mut events = Vec::new();
        self.accept_connections(now);
        self.pump_tcp(now, &mut events);
        self.pump_udp(now, &mut events);
        self.expire(now, &mut events);
        events.truncate(MAX_EVENTS_PER_PUMP);
        events
    }

    fn accept_connections(&mut self, now: f64) {
        loop {
            match self.listener.accept() {
                Ok((stream, addr)) => {
                    if self.pending_connections() >= MAX_PENDING_CONNECTIONS
                        || self.connections.len() >= MAX_PLAYERS + MAX_PENDING_CONNECTIONS
                    {
                        self.stats.rejected_pending += 1;
                        drop(stream);
                        continue;
                    }
                    if let Ok(conn) = Connection::new(stream, addr, now) {
                        self.connections.push(conn);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }

    fn pump_tcp(&mut self, now: f64, events: &mut Vec<HostEvent>) {
        let key = self.config.lobby_key.clone();
        let mut drop_indices = Vec::new();

        for i in 0..self.connections.len() {
            let frames = match self.connections[i].read_frames(&key) {
                Ok(frames) => frames,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => Vec::new(),
                Err(_) => {
                    drop_indices.push(i);
                    continue;
                }
            };
            for (sender, payload) in frames {
                let Ok(msg) = ClientToHost::deserialize_bin(&payload) else {
                    self.stats.malformed += 1;
                    drop_indices.push(i);
                    break;
                };
                if !self.handle_client_message(i, sender, msg, now, events) {
                    drop_indices.push(i);
                    break;
                }
            }
            if self.connections[i].flush().is_err() || self.connections[i].overflowed {
                drop_indices.push(i);
            }
        }

        drop_indices.sort_unstable();
        drop_indices.dedup();
        for i in drop_indices.into_iter().rev() {
            self.drop_connection(i, LeaveReason::Explicit, events);
        }
    }

    /// Returns false when the connection must be dropped.
    fn handle_client_message(
        &mut self,
        index: usize,
        sender: u64,
        msg: ClientToHost,
        now: f64,
        events: &mut Vec<HostEvent>,
    ) -> bool {
        match msg {
            ClientToHost::Join {
                protocol,
                name,
                udp_port,
            } => {
                if protocol != PROTOCOL_VERSION {
                    self.send_to(
                        index,
                        &HostToClient::Bye {
                            reason: LeaveReason::ProtocolMismatch,
                        },
                        PacketClass::Control,
                    );
                    let _ = self.connections[index].flush();
                    return false;
                }
                let player = PlayerId(sender);
                let rejoin = self.players.contains_key(&player);
                if !rejoin && self.players.len() >= self.config.max_players {
                    self.stats.rejected_full += 1;
                    self.send_to(
                        index,
                        &HostToClient::Bye {
                            reason: LeaveReason::LobbyFull,
                        },
                        PacketClass::Control,
                    );
                    let _ = self.connections[index].flush();
                    return false;
                }
                let addr = self.connections[index].addr;
                self.connections[index].player = Some(player);
                // Rejoin resets every per-player counter, including the input
                // tick window — otherwise a returning client whose tick starts
                // over is ignored until it catches up.
                self.players.insert(
                    player,
                    PlayerSlot {
                        name: name.clone(),
                        ip: addr.ip(),
                        // Pinned now so state flows even to a client that never
                        // sends input (a spectator), and so no later datagram
                        // can move it.
                        udp_addr: Some(SocketAddr::new(addr.ip(), udp_port)),
                        last_seen: now,
                        last_input_tick: None,
                    },
                );
                let (tick, snapshot) = self
                    .pending_snapshot
                    .clone()
                    .unwrap_or((0, Vec::new()));
                self.send_to(
                    index,
                    &HostToClient::Welcome {
                        player_id: player,
                        tick,
                        snapshot,
                    },
                    PacketClass::Control,
                );
                let _ = self.connections[index].flush();
                events.push(HostEvent::Joined { player, name });
                true
            }
            ClientToHost::Leave => false,
            ClientToHost::Ping { nonce } => {
                self.send_to(index, &HostToClient::Pong { nonce }, PacketClass::Control);
                true
            }
            ClientToHost::Intent { intent } => {
                let Some(player) = self.connections[index].player else {
                    return false;
                };
                if let Some(slot) = self.players.get_mut(&player) {
                    slot.last_seen = now;
                }
                events.push(HostEvent::Intent { player, intent });
                true
            }
            // Input belongs on UDP; accepting it here too keeps a client that
            // has no working datagram path playable.
            ClientToHost::Input { frame } => {
                let Some(player) = self.connections[index].player else {
                    return false;
                };
                self.apply_input(player, frame, now, events);
                true
            }
        }
    }

    fn apply_input(
        &mut self,
        player: PlayerId,
        frame: InputFrame,
        now: f64,
        events: &mut Vec<HostEvent>,
    ) {
        if !frame.is_finite() {
            self.stats.malformed += 1;
            return;
        }
        let Some(slot) = self.players.get_mut(&player) else {
            return;
        };
        slot.last_seen = now;
        if let Some(last) = slot.last_input_tick {
            if frame.tick <= last {
                self.stats.stale_dropped += 1;
                return;
            }
        }
        slot.last_input_tick = Some(frame.tick);
        events.push(HostEvent::Input { player, frame });
    }

    fn pump_udp(&mut self, now: f64, events: &mut Vec<HostEvent>) {
        let mut buf = [0u8; 2048];
        for _ in 0..UDP_DATAGRAM_BUDGET {
            let (len, src) = match self.udp.recv_from(&mut buf) {
                Ok(v) => v,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            };
            self.stats.datagrams_in += 1;
            self.stats.bytes_in += len as u64;

            // Authenticate before consulting or mutating any peer state.
            let Some((sender, payload)) = Envelope::open(&buf[..len], &self.config.lobby_key)
            else {
                self.stats.auth_failures += 1;
                continue;
            };
            let player = PlayerId(sender);
            let Some(slot) = self.players.get_mut(&player) else {
                continue;
            };
            // Address is pinned to the joining IP and to the first datagram's
            // port; anything else is a spoof attempt, never a rebind.
            match slot.udp_addr {
                Some(known) if known != src => {
                    self.stats.source_mismatches += 1;
                    continue;
                }
                None if src.ip() != slot.ip => {
                    self.stats.source_mismatches += 1;
                    continue;
                }
                None => slot.udp_addr = Some(src),
                Some(_) => {}
            }

            let Ok(msg) = ClientToHost::deserialize_bin(payload) else {
                self.stats.malformed += 1;
                continue;
            };
            match msg {
                ClientToHost::Input { frame } => self.apply_input(player, frame, now, events),
                ClientToHost::Intent { intent } => {
                    events.push(HostEvent::Intent { player, intent })
                }
                // Join/Leave are reliable-channel operations; accepting them
                // from a datagram is exactly the one-packet kick the audit found.
                _ => {}
            }
        }
    }

    fn expire(&mut self, now: f64, events: &mut Vec<HostEvent>) {
        let mut drops = Vec::new();
        for (i, conn) in self.connections.iter().enumerate() {
            if conn.player.is_none() && now - conn.accepted_at > HANDSHAKE_DEADLINE {
                drops.push((i, LeaveReason::Timeout, true));
            }
        }
        for (i, conn) in self.connections.iter().enumerate() {
            if let Some(player) = conn.player {
                if let Some(slot) = self.players.get(&player) {
                    if now - slot.last_seen > PEER_TIMEOUT {
                        drops.push((i, LeaveReason::Timeout, false));
                    }
                }
            }
        }
        drops.sort_unstable_by_key(|(i, _, _)| *i);
        drops.dedup_by_key(|(i, _, _)| *i);
        for (i, reason, handshake) in drops.into_iter().rev() {
            if handshake {
                self.stats.handshake_timeouts += 1;
            }
            self.drop_connection(i, reason, events);
        }
    }

    fn drop_connection(&mut self, index: usize, reason: LeaveReason, events: &mut Vec<HostEvent>) {
        if index >= self.connections.len() {
            return;
        }
        let conn = self.connections.remove(index);
        if let Some(player) = conn.player {
            self.players.remove(&player);
            events.push(HostEvent::Left { player, reason });
        }
    }
}

/// Snapshot handed to newly joining clients so they can reconstruct the world
/// mid-session.
impl Host {
    pub fn set_snapshot(&mut self, tick: u64, entities: Vec<EntityState>) {
        self.pending_snapshot = Some((tick, entities));
    }
}

/// What a client application observes.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientEvent {
    Welcome { player: PlayerId, tick: u64 },
    State { tick: u64 },
    Event { tick: u64, event: GameEvent },
    Disconnected { reason: LeaveReason },
}

/// Client endpoint. Sends intent, receives truth, and reconstructs the world
/// from tick-stamped authoritative state.
pub struct Client {
    client_id: u64,
    key: LobbyKey,
    conn: Connection,
    udp: UdpSocket,
    host_udp: SocketAddr,
    host_id: Option<u64>,
    pub player: Option<PlayerId>,
    /// Latest applied state per entity, keyed by id — the client's view of the
    /// world. Cleared on (re)join.
    pub entities: HashMap<u64, EntityState>,
    entity_seq: HashMap<u64, u32>,
    pub last_tick: u64,
    pub stats: NetStats,
}

impl Client {
    /// Connects to a host. This is the one place a blocking call is acceptable:
    /// it is an explicit user action, bounded by a timeout, for exactly one
    /// socket — never a loop over discovered peers.
    pub fn connect(
        client_id: u64,
        name: &str,
        host_tcp: SocketAddr,
        host_udp: SocketAddr,
        secret: &[u8],
        now: f64,
    ) -> io::Result<Self> {
        let stream = TcpStream::connect_timeout(&host_tcp, Duration::from_millis(500))?;
        let local_ip = stream.local_addr()?.ip();
        let key = LobbyKey::new(secret);
        let udp = bind_udp(SocketAddr::new(local_ip, 0))?;
        let addr = stream.peer_addr()?;
        let mut conn = Connection::new(stream, addr, now)?;

        let join = ClientToHost::Join {
            protocol: PROTOCOL_VERSION,
            name: name.to_string(),
            udp_port: udp.local_addr()?.port(),
        };
        conn.queue(client_id, &join.serialize_bin(), &key, PacketClass::Control);
        conn.flush()?;

        Ok(Self {
            client_id,
            key,
            conn,
            udp,
            host_udp,
            host_id: None,
            player: None,
            entities: HashMap::new(),
            entity_seq: HashMap::new(),
            last_tick: 0,
            stats: NetStats::default(),
        })
    }

    pub fn udp_addr(&self) -> io::Result<SocketAddr> {
        self.udp.local_addr()
    }

    pub fn send_input(&mut self, frame: InputFrame) {
        let msg = ClientToHost::Input { frame };
        let datagram = Envelope::seal(self.client_id, &msg.serialize_bin(), &self.key);
        if self.udp.send_to(&datagram, self.host_udp).is_ok() {
            self.stats.datagrams_out += 1;
            self.stats.bytes_out += datagram.len() as u64;
        }
    }

    pub fn send_intent(&mut self, intent: Intent) {
        let msg = ClientToHost::Intent { intent };
        self.conn.queue(
            self.client_id,
            &msg.serialize_bin(),
            &self.key,
            PacketClass::Control,
        );
    }

    pub fn leave(&mut self) {
        let msg = ClientToHost::Leave;
        self.conn.queue(
            self.client_id,
            &msg.serialize_bin(),
            &self.key,
            PacketClass::Control,
        );
        let _ = self.conn.flush();
    }

    pub fn pump(&mut self, _now: f64) -> Vec<ClientEvent> {
        let mut events = Vec::new();
        match self.conn.read_frames(&self.key) {
            Ok(frames) => {
                for (sender, payload) in frames {
                    if let Ok(msg) = HostToClient::deserialize_bin(&payload) {
                        self.apply_host_message(sender, msg, &mut events);
                    } else {
                        self.stats.malformed += 1;
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => events.push(ClientEvent::Disconnected {
                reason: LeaveReason::Timeout,
            }),
        }
        let _ = self.conn.flush();
        self.pump_udp(&mut events);
        events.truncate(MAX_EVENTS_PER_PUMP);
        events
    }

    fn pump_udp(&mut self, events: &mut Vec<ClientEvent>) {
        let mut buf = [0u8; 2048];
        for _ in 0..UDP_DATAGRAM_BUDGET {
            let (len, _src) = match self.udp.recv_from(&mut buf) {
                Ok(v) => v,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            };
            self.stats.datagrams_in += 1;
            self.stats.bytes_in += len as u64;
            let Some((sender, payload)) = Envelope::open(&buf[..len], &self.key) else {
                self.stats.auth_failures += 1;
                continue;
            };
            let Ok(msg) = HostToClient::deserialize_bin(payload) else {
                self.stats.malformed += 1;
                continue;
            };
            self.apply_host_message(sender, msg, events);
        }
    }

    fn apply_host_message(
        &mut self,
        sender: u64,
        msg: HostToClient,
        events: &mut Vec<ClientEvent>,
    ) {
        // Authoritative messages come from the host and nobody else. With no
        // per-object authority to negotiate, this one check is the whole trust
        // model on the client side.
        match self.host_id {
            Some(known) if known != sender => {
                self.stats.source_mismatches += 1;
                return;
            }
            _ => {}
        }

        match msg {
            HostToClient::Welcome {
                player_id,
                tick,
                snapshot,
            } => {
                self.host_id = Some(sender);
                self.player = Some(player_id);
                // A (re)join replaces the world wholesale, so stale sequence
                // state from a previous session cannot suppress fresh updates.
                self.entities.clear();
                self.entity_seq.clear();
                self.last_tick = tick;
                for state in snapshot {
                    if state.is_finite() {
                        self.entity_seq.insert(state.id, state.seq);
                        self.entities.insert(state.id, state);
                    }
                }
                events.push(ClientEvent::Welcome {
                    player: player_id,
                    tick,
                });
            }
            HostToClient::StateBatch { tick, entities } => {
                for state in entities {
                    if !state.is_finite() {
                        self.stats.malformed += 1;
                        continue;
                    }
                    // Per-entity sequencing: a late or duplicated datagram can
                    // only be ignored for the entities it actually carries.
                    match self.entity_seq.get(&state.id) {
                        Some(&seen) if seq_is_stale(seen, state.seq) => {
                            self.stats.stale_dropped += 1;
                            continue;
                        }
                        _ => {}
                    }
                    self.entity_seq.insert(state.id, state.seq);
                    self.entities.insert(state.id, state);
                }
                self.last_tick = self.last_tick.max(tick);
                events.push(ClientEvent::State { tick });
            }
            HostToClient::Event { tick, event } => {
                match &event {
                    GameEvent::Spawn { id, state, .. } => {
                        if state.is_finite() {
                            self.entity_seq.insert(*id, state.seq);
                            self.entities.insert(*id, *state);
                        }
                    }
                    GameEvent::Remove { id } => {
                        self.entities.remove(id);
                        self.entity_seq.remove(id);
                    }
                    _ => {}
                }
                self.last_tick = self.last_tick.max(tick);
                events.push(ClientEvent::Event { tick, event });
            }
            HostToClient::Bye { reason } => events.push(ClientEvent::Disconnected { reason }),
            HostToClient::Pong { .. } => {}
        }
    }
}

/// Wrapping-safe staleness test over a 32-bit sequence space.
fn seq_is_stale(seen: u32, incoming: u32) -> bool {
    incoming == seen || incoming.wrapping_sub(seen) > u32::MAX / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_staleness_handles_wraparound() {
        assert!(seq_is_stale(5, 4));
        assert!(seq_is_stale(5, 5));
        assert!(!seq_is_stale(5, 6));
        // Wrapping forward is fresh, wrapping backward is stale.
        assert!(!seq_is_stale(u32::MAX, 0));
        assert!(seq_is_stale(0, u32::MAX));
    }
}
