//! [`VoiceLink`]: the socket, the peer discovery, and the control surface.
//!
//! One UDP socket does everything (port 41531 by default — keep it fixed:
//! LAN firewalls whitelist it). Presence is a 24-byte HELLO broadcast twice a
//! second to the local subnet (and to every port in a small range, so several
//! instances on one machine find each other); audio is unicast to each
//! discovered peer by default, or broadcast with [`Delivery::Broadcast`].
//! Peers are keyed by the `sender` id in every packet, expire after silence,
//! and say goodbye with a BYE packet on drop.

use crate::capture::CaptureHandle;
use crate::codec;
use crate::jitter::PlayoutConfig;
use crate::peers::{pack_addr, unpack_addr, PeerTable};
use crate::playback::PlaybackHandle;
use crate::wire::{decode_raw_i16, encode_header_only, flags, Codec, Header, MAX_FRAME};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How audio frames leave the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// One datagram per discovered peer (default). Works across WiFi and
    /// never bothers hosts that are not in the chat.
    Unicast,
    /// One datagram to the subnet broadcast address. Cheapest for many
    /// peers on a wired switch; assumes one instance per host on the base
    /// port, and WiFi handles broadcast badly.
    Broadcast,
}

/// Everything [`VoiceLink::bind`] needs. `Default` is a working LAN setup.
#[derive(Clone, Debug)]
pub struct VoiceConfig {
    /// UDP port to bind and to discover on. 0 = ephemeral (tests).
    pub port: u16,
    /// If the port is taken (another instance on this host), try up to this
    /// many following ports; HELLOs cover the whole range.
    pub port_range: u16,
    /// Addresses to always send HELLOs to (peers outside the broadcast
    /// domain, or test instances).
    pub static_peers: Vec<SocketAddr>,
    pub delivery: Delivery,
    /// Send discovery HELLOs to the subnet broadcast address. Off for tests.
    pub broadcast: bool,
    /// Override the broadcast addresses (default: the /24 of the primary
    /// interface, plus 255.255.255.255).
    pub broadcast_addrs: Vec<Ipv4Addr>,
    /// Samples per frame at 48 kHz: 240 = 5 ms (default), 480 = 10 ms,
    /// 120 = 2.5 ms. Smaller = lower latency, more packets.
    pub frame_samples: usize,
    /// What audio frames carry: [`Codec::RawI16`] (transparent, 807 kbit/s
    /// at 5 ms frames) or [`Codec::Ogg`] (vendored ADPCM-in-Ogg,
    /// ≈ 300 kbit/s at 4 bits). Receivers follow each packet's codec id, so
    /// mixed senders are fine.
    pub codec: Codec,
    /// ADPCM quantiser depth for [`Codec::Ogg`]: 4 (default), 3, or 2.
    pub adpcm_bits: u8,
    /// Room tag: only links bound to the same room hear each other — two
    /// sessions on one LAN stay separate, and `sender_id` values (including
    /// the [`crate::wire::HOST_SENDER_ID`] sentinel) only need to be unique
    /// within a room. Derive it from the session secret (the sandbox uses
    /// the first 8 bytes of `LobbyKey::mac(b"makepad-voice-room")`).
    /// 0 = the default "public" room.
    pub room: u64,
    /// Identity stamped on every packet; receivers key peers on it and hand
    /// it back per rendered frame. In a game session this is the net player
    /// id, with the HOST as [`crate::wire::HOST_SENDER_ID`] (`u64::MAX`).
    /// 0 = random (standalone use).
    pub sender_id: u64,
    /// Team channel to send on: 0 = everyone hears it.
    pub channel: u8,
    /// Voice gate threshold (RMS, linear). <= 0 disables the gate (always
    /// send audio).
    pub gate_threshold_rms: f32,
    /// How long the gate stays open after the last audible frame.
    pub gate_hangover_ms: u32,
    /// Jitter buffer tuning.
    pub playout: PlayoutConfig,
    /// Forget a peer after this long without any packet.
    pub peer_timeout_ms: u64,
    /// Presence interval.
    pub hello_ms: u64,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            port: 41531,
            port_range: 8,
            static_peers: Vec::new(),
            delivery: Delivery::Unicast,
            broadcast: true,
            broadcast_addrs: Vec::new(),
            frame_samples: 240,
            codec: Codec::RawI16,
            adpcm_bits: 4,
            room: 0,
            sender_id: 0,
            channel: 0,
            gate_threshold_rms: 0.003,
            gate_hangover_ms: 300,
            playout: PlayoutConfig::default(),
            peer_timeout_ms: 3000,
            hello_ms: 500,
        }
    }
}

#[derive(Default)]
pub(crate) struct Stats {
    pub packets_sent: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub packets_recv: AtomicU64,
    pub bytes_recv: AtomicU64,
    pub bad_packets: AtomicU64,
    pub own_loopback: AtomicU64,
    pub filtered: AtomicU64,
    pub send_errors: AtomicU64,
    pub hellos_sent: AtomicU64,
    pub peer_table_full: AtomicU64,
    pub opaque_payloads: AtomicU64,
    pub wrong_room: AtomicU64,
}

/// A snapshot of the link counters.
#[derive(Clone, Copy, Debug, Default)]
pub struct LinkStats {
    pub packets_sent: u64,
    pub bytes_sent: u64,
    pub packets_recv: u64,
    pub bytes_recv: u64,
    pub bad_packets: u64,
    /// Own broadcast packets seen and dropped (normal in broadcast mode).
    pub own_loopback: u64,
    /// Audio dropped by the team-channel filter.
    pub filtered: u64,
    pub send_errors: u64,
    pub hellos_sent: u64,
    pub peer_table_full: u64,
    /// Frames whose payload could not be decoded (corrupt page or unknown
    /// codec); each kept its timing as silence.
    pub opaque_payloads: u64,
    /// Packets from a different room, dropped before any peer state.
    pub wrong_room: u64,
    pub active_peers: usize,
}

/// A snapshot of one peer, for UIs and for wiring voices to entities.
#[derive(Clone, Copy, Debug)]
pub struct PeerInfo {
    pub sender: u64,
    pub addr: Option<SocketAddr>,
    /// Team channel the peer currently sends on.
    pub channel: u8,
    pub talking: bool,
    pub gain: f32,
    pub muted: bool,
    pub packets: u64,
    /// Milliseconds since the last packet.
    pub silent_for_ms: u64,
    /// Audio buffered ahead of playback, in ms.
    pub buffered_ms: f32,
    /// Jitter buffer target, in frames.
    pub target_frames: u32,
    pub frames_accepted: u32,
    pub frames_late: u32,
    pub frames_duplicate: u32,
}

pub(crate) struct Shared {
    pub frame_samples: usize,
    pub playout: PlayoutConfig,
    pub gate_threshold_rms: f32,
    pub gate_hangover_ms: u32,
    pub room: AtomicU64,
    pub wire_codec: AtomicU8,
    pub adpcm_bits: AtomicU8,
    pub sender_id: AtomicU64,
    pub channel: AtomicU8,
    pub listen_all: AtomicBool,
    pub listen_mask: [AtomicU64; 4],
    pub muted: AtomicBool,
    pub input_gain: AtomicU32,
    pub output_gain: AtomicU32,
    pub delivery_mode: AtomicU8,
    /// Packed IPv4:port audio broadcast targets (Delivery::Broadcast).
    pub broadcast_targets: [AtomicU64; 4],
    pub peers: PeerTable,
    pub stats: Stats,
    pub running: AtomicBool,
    /// HELLO destinations added after bind ([`VoiceLink::add_peer`]).
    pub extra_targets: Mutex<Vec<SocketAddr>>,
    epoch: Instant,
}

impl Shared {
    pub fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    pub fn delivery(&self) -> Delivery {
        if self.delivery_mode.load(Ordering::Relaxed) == 1 {
            Delivery::Broadcast
        } else {
            Delivery::Unicast
        }
    }

    /// The team-channel filter: channel 0 always plays; otherwise the
    /// listen set decides.
    pub fn accepts_channel(&self, channel: u8) -> bool {
        if channel == 0 || self.listen_all.load(Ordering::Relaxed) {
            return true;
        }
        let word = self.listen_mask[(channel / 64) as usize].load(Ordering::Relaxed);
        word & (1u64 << (channel % 64)) != 0
    }
}

fn random_id() -> u64 {
    use std::hash::{BuildHasher, Hash, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut h);
    std::process::id().hash(&mut h);
    match h.finish() {
        0 => 1,
        crate::wire::HOST_SENDER_ID => crate::wire::HOST_SENDER_ID - 1,
        v => v,
    }
}

/// The primary interface's IPv4 address, learned without sending a packet
/// (UDP connect only sets the route).
fn primary_local_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    let _ = socket.set_broadcast(true);
    for target in ["255.255.255.255:41531", "8.8.8.8:53"] {
        if socket.connect(target).is_ok() {
            if let Ok(SocketAddr::V4(local)) = socket.local_addr() {
                let ip = *local.ip();
                if !ip.is_loopback() && !ip.is_unspecified() {
                    return Some(ip);
                }
            }
        }
    }
    None
}

/// A LAN voice link. Bind it once; move [`CaptureHandle`] into the audio
/// input callback and [`PlaybackHandle`] into the audio output callback;
/// keep the link itself for control (channels, gains, mute, peers, stats).
/// Dropping it says BYE and stops the network thread; handles already moved
/// into callbacks stay safe (they just go quiet).
pub struct VoiceLink {
    shared: Arc<Shared>,
    local_addr: SocketAddr,
    capture: Option<CaptureHandle>,
    playback: Option<PlaybackHandle>,
    rx_thread: Option<std::thread::JoinHandle<()>>,
}

impl VoiceLink {
    pub fn bind(mut config: VoiceConfig) -> io::Result<VoiceLink> {
        if config.frame_samples < 32 || config.frame_samples > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("frame_samples must be 32..={MAX_FRAME}"),
            ));
        }
        config.playout.default_frame = config.frame_samples;

        // Bind the port, or the next one in the range if taken.
        let mut socket = None;
        let mut last_err = None;
        let tries = if config.port == 0 { 0 } else { config.port_range };
        for i in 0..=tries {
            match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, config.port.wrapping_add(i))) {
                Ok(s) => {
                    socket = Some(s);
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }
        let socket = match socket {
            Some(s) => s,
            None => return Err(last_err.unwrap()),
        };
        socket.set_broadcast(true)?;
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        let local_addr = socket.local_addr()?;

        let sender_id = if config.sender_id != 0 {
            config.sender_id
        } else {
            random_id()
        };

        // Discovery targets.
        let mut bcast_ips: Vec<Ipv4Addr> = config.broadcast_addrs.clone();
        if bcast_ips.is_empty() && config.broadcast {
            if let Some(ip) = primary_local_ipv4() {
                let o = ip.octets();
                bcast_ips.push(Ipv4Addr::new(o[0], o[1], o[2], 255));
            }
            bcast_ips.push(Ipv4Addr::BROADCAST);
        }
        if !config.broadcast {
            bcast_ips.clear();
        }
        let base_port = if config.port == 0 {
            local_addr.port()
        } else {
            config.port
        };
        let mut hello_targets: Vec<SocketAddr> = Vec::new();
        for &ip in &bcast_ips {
            for p in 0..=config.port_range {
                hello_targets.push(SocketAddr::new(IpAddr::V4(ip), base_port.wrapping_add(p)));
            }
        }
        hello_targets.extend(config.static_peers.iter().copied());

        let shared = Arc::new(Shared {
            frame_samples: config.frame_samples,
            playout: config.playout,
            gate_threshold_rms: config.gate_threshold_rms,
            gate_hangover_ms: config.gate_hangover_ms,
            room: AtomicU64::new(config.room),
            wire_codec: AtomicU8::new(config.codec as u8),
            adpcm_bits: AtomicU8::new(config.adpcm_bits.clamp(2, 4)),
            sender_id: AtomicU64::new(sender_id),
            channel: AtomicU8::new(config.channel),
            listen_all: AtomicBool::new(true),
            listen_mask: Default::default(),
            muted: AtomicBool::new(false),
            input_gain: AtomicU32::new(1.0f32.to_bits()),
            output_gain: AtomicU32::new(1.0f32.to_bits()),
            delivery_mode: AtomicU8::new(match config.delivery {
                Delivery::Unicast => 0,
                Delivery::Broadcast => 1,
            }),
            broadcast_targets: Default::default(),
            peers: PeerTable::new(),
            stats: Stats::default(),
            running: AtomicBool::new(true),
            extra_targets: Mutex::new(Vec::new()),
            epoch: Instant::now(),
        });
        for (i, &ip) in bcast_ips.iter().take(4).enumerate() {
            shared.broadcast_targets[i].store(
                pack_addr(SocketAddr::new(IpAddr::V4(ip), base_port)),
                Ordering::Relaxed,
            );
        }

        let rx_socket = socket.try_clone()?;
        let rx_shared = shared.clone();
        let hello_ms = config.hello_ms.max(100);
        let peer_timeout_ms = config.peer_timeout_ms.max(500);
        let rx_thread = std::thread::Builder::new()
            .name("teamtalk-rx".into())
            .spawn(move || rx_loop(rx_shared, rx_socket, hello_targets, hello_ms, peer_timeout_ms))?;

        Ok(VoiceLink {
            capture: Some(CaptureHandle::new(shared.clone(), socket.try_clone()?)),
            playback: Some(PlaybackHandle::new(shared.clone())),
            shared,
            local_addr,
            rx_thread: Some(rx_thread),
        })
    }

    /// The bound address (port matters: it is what peers discover).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The identity stamped on outgoing packets.
    pub fn sender_id(&self) -> u64 {
        self.shared.sender_id.load(Ordering::Relaxed)
    }

    /// Change the identity (e.g. once the game assigns a player id; the
    /// HOST uses [`crate::wire::HOST_SENDER_ID`]).
    pub fn set_sender_id(&self, id: u64) {
        self.shared.sender_id.store(id.max(1), Ordering::Relaxed);
    }

    /// Switch what outgoing audio frames carry. Takes effect on the next
    /// frame; receivers follow the per-packet codec id, so this is safe
    /// mid-stream.
    pub fn set_codec(&self, codec: Codec) {
        self.shared.wire_codec.store(codec as u8, Ordering::Relaxed);
    }

    pub fn codec(&self) -> Codec {
        Codec::from_u8(self.shared.wire_codec.load(Ordering::Relaxed)).unwrap_or(Codec::RawI16)
    }

    /// ADPCM depth for [`Codec::Ogg`]: 4, 3 or 2 bits per sample.
    pub fn set_adpcm_bits(&self, bits: u8) {
        self.shared
            .adpcm_bits
            .store(bits.clamp(2, 4), Ordering::Relaxed);
    }

    /// The room this link is bound to.
    pub fn room(&self) -> u64 {
        self.shared.room.load(Ordering::Relaxed)
    }

    /// Move to another room. Peers from the old room stop being heard at
    /// once (their packets fail the room check) and expire from the table.
    pub fn set_room(&self, room: u64) {
        self.shared.room.store(room, Ordering::Relaxed);
    }

    /// The capture half; `Some` exactly once.
    pub fn take_capture(&mut self) -> Option<CaptureHandle> {
        self.capture.take()
    }

    /// The playback half; `Some` exactly once.
    pub fn take_playback(&mut self) -> Option<PlaybackHandle> {
        self.playback.take()
    }

    /// Team channel to send on (0 = everyone).
    pub fn set_channel(&self, channel: u8) {
        self.shared.channel.store(channel, Ordering::Relaxed);
    }

    pub fn channel(&self) -> u8 {
        self.shared.channel.load(Ordering::Relaxed)
    }

    /// Hear every channel (the default).
    pub fn set_listen_all(&self) {
        self.shared.listen_all.store(true, Ordering::Relaxed);
    }

    /// Hear only these channels (channel-0 audio always plays).
    pub fn set_listen_channels(&self, channels: &[u8]) {
        let mut mask = [0u64; 4];
        for &c in channels {
            mask[(c / 64) as usize] |= 1u64 << (c % 64);
        }
        for (word, &m) in self.shared.listen_mask.iter().zip(&mask) {
            word.store(m, Ordering::Relaxed);
        }
        self.shared.listen_all.store(false, Ordering::Relaxed);
    }

    /// Would audio on `channel` play here?
    pub fn listens_to(&self, channel: u8) -> bool {
        self.shared.accepts_channel(channel)
    }

    /// Stop sending audio (silence packets keep presence alive).
    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    pub fn muted(&self) -> bool {
        self.shared.muted.load(Ordering::Relaxed)
    }

    /// Microphone gain before the gate (1.0 = unity).
    pub fn set_input_gain(&self, gain: f32) {
        self.shared
            .input_gain
            .store(gain.clamp(0.0, 16.0).to_bits(), Ordering::Relaxed);
    }

    /// Master gain on the received voice mix ("others volume").
    pub fn set_output_gain(&self, gain: f32) {
        self.shared
            .output_gain
            .store(gain.clamp(0.0, 16.0).to_bits(), Ordering::Relaxed);
    }

    /// Playback gain for one peer.
    pub fn set_peer_gain(&self, sender: u64, gain: f32) {
        if let Some(i) = self.shared.peers.find(sender) {
            self.shared.peers.slots()[i]
                .gain
                .store(gain.clamp(0.0, 16.0).to_bits(), Ordering::Relaxed);
        }
    }

    /// Locally mute one peer.
    pub fn set_peer_muted(&self, sender: u64, muted: bool) {
        if let Some(i) = self.shared.peers.find(sender) {
            self.shared.peers.slots()[i]
                .muted
                .store(muted, Ordering::Relaxed);
        }
    }

    /// Also send HELLOs here (a peer outside the broadcast domain).
    pub fn add_peer(&self, addr: SocketAddr) {
        if let Ok(mut extra) = self.shared.extra_targets.lock() {
            if !extra.contains(&addr) {
                extra.push(addr);
            }
        }
    }

    /// Switch between unicast and broadcast audio delivery.
    pub fn set_delivery(&self, delivery: Delivery) {
        self.shared.delivery_mode.store(
            match delivery {
                Delivery::Unicast => 0,
                Delivery::Broadcast => 1,
            },
            Ordering::Relaxed,
        );
    }

    /// Snapshot of everyone currently heard from. (Allocates: control
    /// thread only, not for the audio callback.)
    pub fn peers(&self) -> Vec<PeerInfo> {
        let now = self.shared.now_ms();
        self.shared
            .peers
            .slots()
            .iter()
            .filter(|s| s.is_active())
            .map(|s| PeerInfo {
                sender: s.sender.load(Ordering::Relaxed),
                addr: unpack_addr(s.addr.load(Ordering::Relaxed)),
                channel: s.channel.load(Ordering::Relaxed),
                talking: s.talking.load(Ordering::Relaxed),
                gain: f32::from_bits(s.gain.load(Ordering::Relaxed)),
                muted: s.muted.load(Ordering::Relaxed),
                packets: s.packets.load(Ordering::Relaxed),
                silent_for_ms: now.saturating_sub(s.last_seen_ms.load(Ordering::Relaxed)),
                buffered_ms: s.buffered_ms_x10.load(Ordering::Relaxed) as f32 / 10.0,
                target_frames: s.target_frames.load(Ordering::Relaxed),
                frames_accepted: s.ring.accepted.load(Ordering::Relaxed),
                frames_late: s.ring.late.load(Ordering::Relaxed),
                frames_duplicate: s.ring.duplicate.load(Ordering::Relaxed),
            })
            .collect()
    }

    /// Snapshot of the link counters.
    pub fn stats(&self) -> LinkStats {
        let s = &self.shared.stats;
        LinkStats {
            packets_sent: s.packets_sent.load(Ordering::Relaxed),
            bytes_sent: s.bytes_sent.load(Ordering::Relaxed),
            packets_recv: s.packets_recv.load(Ordering::Relaxed),
            bytes_recv: s.bytes_recv.load(Ordering::Relaxed),
            bad_packets: s.bad_packets.load(Ordering::Relaxed),
            own_loopback: s.own_loopback.load(Ordering::Relaxed),
            filtered: s.filtered.load(Ordering::Relaxed),
            send_errors: s.send_errors.load(Ordering::Relaxed),
            hellos_sent: s.hellos_sent.load(Ordering::Relaxed),
            peer_table_full: s.peer_table_full.load(Ordering::Relaxed),
            opaque_payloads: s.opaque_payloads.load(Ordering::Relaxed),
            wrong_room: s.wrong_room.load(Ordering::Relaxed),
            active_peers: self.shared.peers.active_count(),
        }
    }
}

impl Drop for VoiceLink {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.rx_thread.take() {
            let _ = t.join();
        }
    }
}

fn send_presence(
    shared: &Shared,
    socket: &UdpSocket,
    targets: &[SocketAddr],
    flag: u8,
) -> u64 {
    let header = Header {
        codec: Codec::RawI16,
        channel: shared.channel.load(Ordering::Relaxed),
        flags: flag,
        frames: 0,
        room: shared.room.load(Ordering::Relaxed),
        sender: shared.sender_id.load(Ordering::Relaxed),
        seq: 0,
        timestamp: 0,
    };
    let mut packet = [0u8; crate::wire::MAX_PACKET];
    let len = encode_header_only(header, &mut packet);
    let mut sent = 0;
    for t in targets {
        if socket.send_to(&packet[..len], t).is_ok() {
            sent += 1;
        }
    }
    sent
}

fn rx_loop(
    shared: Arc<Shared>,
    socket: UdpSocket,
    mut hello_targets: Vec<SocketAddr>,
    hello_ms: u64,
    peer_timeout_ms: u64,
) {
    let mut buf = [0u8; 2048];
    let mut pcm = [0i16; MAX_FRAME];
    let mut last_hello: Option<u64> = None;
    let mut last_sweep = 0u64;
    let mut round_targets: Vec<SocketAddr> = Vec::new();

    while shared.running.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((len, addr)) => handle_packet(&shared, &buf[..len], addr, &mut pcm),
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {}
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
        let now = shared.now_ms();
        if last_hello.map_or(true, |t| now.saturating_sub(t) >= hello_ms) {
            last_hello = Some(now);
            if let Ok(mut extra) = shared.extra_targets.lock() {
                for a in extra.drain(..) {
                    if !hello_targets.contains(&a) {
                        hello_targets.push(a);
                    }
                }
            }
            // Presence goes to the configured targets AND to everyone we
            // have discovered: that keeps the link symmetric even when only
            // one side knows the other's address up front.
            round_targets.clear();
            round_targets.extend_from_slice(&hello_targets);
            for slot in shared.peers.slots() {
                if slot.is_active() {
                    if let Some(a) = unpack_addr(slot.addr.load(Ordering::Relaxed)) {
                        if !round_targets.contains(&a) {
                            round_targets.push(a);
                        }
                    }
                }
            }
            let sent = send_presence(&shared, &socket, &round_targets, flags::HELLO);
            shared.stats.hellos_sent.fetch_add(sent, Ordering::Relaxed);
        }
        if now.saturating_sub(last_sweep) >= 500 {
            last_sweep = now;
            shared.peers.expire(now, peer_timeout_ms);
        }
    }

    // Goodbye: to the discovered peers and the discovery targets.
    let mut targets = hello_targets;
    for slot in shared.peers.slots() {
        if slot.is_active() {
            if let Some(a) = unpack_addr(slot.addr.load(Ordering::Relaxed)) {
                if !targets.contains(&a) {
                    targets.push(a);
                }
            }
        }
    }
    send_presence(&shared, &socket, &targets, flags::BYE);
}

fn handle_packet(shared: &Shared, buf: &[u8], addr: SocketAddr, pcm: &mut [i16; MAX_FRAME]) {
    shared.stats.packets_recv.fetch_add(1, Ordering::Relaxed);
    shared
        .stats
        .bytes_recv
        .fetch_add(buf.len() as u64, Ordering::Relaxed);
    let (header, payload) = match Header::parse(buf) {
        Ok(v) => v,
        Err(_) => {
            shared.stats.bad_packets.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    if header.room != shared.room.load(Ordering::Relaxed) {
        // A different session on this LAN: drop before the packet touches
        // the peer table or any jitter buffer — sender ids (including the
        // HOST sentinel) are only unique within a room.
        shared.stats.wrong_room.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if header.sender == shared.sender_id.load(Ordering::Relaxed) {
        // Our own broadcast came back around.
        shared.stats.own_loopback.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if header.is_bye() {
        shared.peers.remove(header.sender);
        return;
    }
    let now = shared.now_ms();
    let Some(idx) = shared.peers.find_or_insert(header.sender, now) else {
        shared.stats.peer_table_full.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let slot = &shared.peers.slots()[idx];
    slot.last_seen_ms.store(now, Ordering::Relaxed);
    let packed = pack_addr(addr);
    if packed != 0 {
        slot.addr.store(packed, Ordering::Relaxed);
    }
    slot.channel.store(header.channel, Ordering::Relaxed);
    slot.packets.fetch_add(1, Ordering::Relaxed);
    if header.is_hello() || !header.has_frame() {
        return;
    }
    if !shared.accepts_channel(header.channel) {
        shared.stats.filtered.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let frames = header.frames as usize;
    if header.is_silence() {
        slot.ring.insert(header.seq, &pcm[..frames], true);
        return;
    }
    match header.codec {
        Codec::RawI16 => {
            let n = decode_raw_i16(payload, &mut pcm[..frames]);
            slot.ring.insert(header.seq, &pcm[..n], false);
        }
        Codec::Ogg => {
            // The vendored ADPCM-in-Ogg codec. Every page is self-contained
            // (its packet carries the decoder entry state), so this decode
            // needs nothing from earlier packets. A payload that fails the
            // CRC or does not carry exactly the frame's samples keeps its
            // timing as silence instead of warping the stream.
            match codec::voice_decode(codec::VoiceCodec::Ogg, payload) {
                Some(samples) if samples.len() == frames => {
                    for (i, &s) in samples.iter().enumerate() {
                        pcm[i] = (s * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
                    }
                    slot.ring.insert(header.seq, &pcm[..frames], false);
                }
                _ => {
                    shared.stats.opaque_payloads.fetch_add(1, Ordering::Relaxed);
                    slot.ring.insert(header.seq, &pcm[..frames], true);
                }
            }
        }
    }
}
