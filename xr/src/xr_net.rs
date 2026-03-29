use crate::*;
use makepad_widgets::makepad_platform::makepad_micro_serde::*;
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const XR_NET_PROTOCOL_VERSION: u16 = 3;
pub const XR_NET_DEFAULT_DISCOVERY_PORT: u16 = 41546;
pub const XR_NET_DEFAULT_DATA_PORT: u16 = 41547;
pub const XR_NET_DEFAULT_SYNC_PORT: u16 = 41548;

const XR_NET_DEFAULT_DISCOVERY_INTERVAL: Duration = Duration::from_millis(100);
const XR_NET_DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const XR_NET_DEFAULT_PEER_TIMEOUT: Duration = Duration::from_secs(2);
const XR_NET_DEFAULT_SYNC_CONNECT_RETRY: Duration = Duration::from_millis(250);
const XR_NET_SYNC_CONNECT_TIMEOUT: Duration = Duration::from_millis(120);
const XR_NET_SYNC_READ_BUDGET_BYTES_PER_POLL: usize = 256 * 1024;
const XR_NET_SYNC_WRITE_BUDGET_BYTES_PER_POLL: usize = 256 * 1024;
// Alignment descriptors now carry full f32 heightmaps, so the old 256 KiB cap
// is too small once the published map grows beyond modest extents.
const XR_NET_SYNC_MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, SerBin, DeBin)]
pub struct XrNetPeerId(pub u64);

impl XrNetPeerId {
    pub fn to_live_id(self) -> LiveId {
        LiveId(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct XrNetPeer {
    pub id: XrNetPeerId,
    pub addr: SocketAddr,
}

impl XrNetPeer {
    pub fn to_live_id(self) -> LiveId {
        self.id.to_live_id()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XrNetLeaveReason {
    Explicit,
    Timeout,
}

#[derive(Clone, Debug, SerBin, DeBin)]
pub struct XrNetStateFrame {
    pub seq: u32,
    pub sent_at: f64,
    pub state: XrState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, SerBin, DeBin)]
pub struct XrNetAlignmentFrame {
    pub seq: u32,
    pub sent_at: f64,
    pub anchor: XrAnchor,
    pub confidence: f32,
}

impl XrNetAlignmentFrame {
    pub fn remote_to_local_transform(
        local: &XrNetAlignmentFrame,
        remote: &XrNetAlignmentFrame,
    ) -> Option<Mat4f> {
        if local.confidence <= 0.0 || remote.confidence <= 0.0 {
            return None;
        }
        Some(Mat4f::mul(
            &local.anchor.to_pose().to_mat4(),
            &remote.anchor.to_pose().to_mat4().invert(),
        ))
    }
}

#[derive(Clone, Debug, Default, PartialEq, SerBin, DeBin)]
pub struct XrNetAlignmentDescriptorFrame {
    pub seq: u32,
    pub sent_at: f64,
    pub descriptor: XrDepthAlignDescriptor,
}

impl XrNetAlignmentDescriptorFrame {
    pub fn from_tsdf_snapshot(snapshot: &TsdfPublishedSnapshot, sent_at: f64) -> Option<Self> {
        let height_map = snapshot.height_map.clone()?;
        let floor_y = height_map.floor_y_meters;
        Some(Self {
            seq: 0,
            sent_at,
            descriptor: XrDepthAlignDescriptor {
                voxel_size_meters: snapshot.grid.voxel_size,
                floor_y,
                wall_normal_histogram: Vec::new(),
                samples: Vec::new(),
                vertical_descriptor: None,
                height_map: Some(height_map),
            },
        })
    }

    pub fn transformed(&self, transform: &Mat4f) -> Self {
        Self {
            descriptor: xr_depth_align_transform_descriptor(&self.descriptor, transform),
            ..self.clone()
        }
    }

    pub fn test_markers(&self) -> Option<[Vec3f; 2]> {
        xr_depth_align_test_markers(&self.descriptor)
    }

    pub fn solve_remote_to_local(
        local: &XrNetAlignmentDescriptorFrame,
        remote: &XrNetAlignmentDescriptorFrame,
    ) -> Option<XrNetAlignmentSolution> {
        xr_depth_align_solve_remote_to_local(&local.descriptor, &remote.descriptor)
    }
}

pub type XrNetAlignmentSolution = XrDepthAlignSolution;

#[derive(Clone, Debug, Default, PartialEq, SerBin, DeBin)]
pub struct XrNetAlignmentDescriptorDumpPair {
    pub format_version: u32,
    pub captured_at_unix_ms: u64,
    pub remote_peer_id: XrNetPeerId,
    pub local_descriptor: XrNetAlignmentDescriptorFrame,
    pub remote_descriptor: XrNetAlignmentDescriptorFrame,
}

impl XrNetAlignmentDescriptorDumpPair {
    pub const FORMAT_VERSION: u32 = 2;
    pub const FILE_MAGIC: &[u8; 8] = b"MXRPAIR1";

    pub fn new(
        remote_peer_id: XrNetPeerId,
        local_descriptor: XrNetAlignmentDescriptorFrame,
        remote_descriptor: XrNetAlignmentDescriptorFrame,
    ) -> Self {
        let captured_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        Self {
            format_version: Self::FORMAT_VERSION,
            captured_at_unix_ms,
            remote_peer_id,
            local_descriptor,
            remote_descriptor,
        }
    }

    pub fn to_file_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::FILE_MAGIC.len() + 512);
        bytes.extend_from_slice(Self::FILE_MAGIC);
        bytes.extend(self.serialize_bin());
        bytes
    }

    pub fn from_file_bytes(bytes: &[u8]) -> Option<Self> {
        if !bytes.starts_with(Self::FILE_MAGIC) {
            return None;
        }
        Self::deserialize_bin(&bytes[Self::FILE_MAGIC.len()..]).ok()
    }
}

pub fn tsdf_snapshot_height_map_preview(
    snapshot: &TsdfPublishedSnapshot,
) -> Option<XrDepthAlignSlicePreview> {
    let height_map = snapshot.height_map.clone()?;
    Some(XrDepthAlignSlicePreview {
        cutout_center: height_map.player_cutout_center,
        cutout_forward: None,
        cutout_radius_meters: height_map.player_cutout_radius_meters,
        height_map,
    })
}

#[derive(Clone, Debug)]
pub enum XrNetIncoming {
    Join {
        peer: XrNetPeer,
    },
    Leave {
        peer: XrNetPeer,
        reason: XrNetLeaveReason,
    },
    State {
        peer: XrNetPeer,
        frame: XrNetStateFrame,
    },
    Alignment {
        peer: XrNetPeer,
        frame: XrNetAlignmentFrame,
    },
    AlignmentDescriptor {
        peer: XrNetPeer,
        frame: XrNetAlignmentDescriptorFrame,
    },
}

#[derive(Clone, Debug)]
pub struct XrNetConfig {
    pub node_id: XrNetPeerId,
    pub discovery_bind: SocketAddr,
    pub data_bind: SocketAddr,
    pub sync_bind: SocketAddr,
    pub discovery_targets: Vec<SocketAddr>,
    pub discovery_interval: Duration,
    pub peer_timeout: Duration,
    pub poll_interval: Duration,
}

impl Default for XrNetConfig {
    fn default() -> Self {
        Self {
            node_id: XrNetPeerId(default_node_id()),
            discovery_bind: SocketAddr::new(
                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                XR_NET_DEFAULT_DISCOVERY_PORT,
            ),
            data_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), XR_NET_DEFAULT_DATA_PORT),
            sync_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), XR_NET_DEFAULT_SYNC_PORT),
            discovery_targets: vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)),
                XR_NET_DEFAULT_DISCOVERY_PORT,
            )],
            discovery_interval: XR_NET_DEFAULT_DISCOVERY_INTERVAL,
            peer_timeout: XR_NET_DEFAULT_PEER_TIMEOUT,
            poll_interval: XR_NET_DEFAULT_POLL_INTERVAL,
        }
    }
}

#[derive(Clone, Debug, SerBin, DeBin)]
struct XrNetDiscoveryHello {
    version: u16,
    node_id: XrNetPeerId,
    data_port: u16,
    sync_port: u16,
}

#[derive(Clone, Debug, SerBin, DeBin)]
struct XrNetLeavePacket {
    version: u16,
    node_id: XrNetPeerId,
}

#[derive(Clone, Debug, SerBin, DeBin)]
enum XrNetDiscoveryPacket {
    Hello(XrNetDiscoveryHello),
    Leave(XrNetLeavePacket),
}

#[derive(Clone, Debug, SerBin, DeBin)]
struct XrNetSyncHello {
    version: u16,
    node_id: XrNetPeerId,
}

#[derive(Clone, Debug, SerBin, DeBin)]
enum XrNetSyncPacket {
    Hello(XrNetSyncHello),
    Data(XrNetDataPacket),
}

#[derive(Clone, Debug, SerBin, DeBin)]
enum XrNetDataPacket {
    State {
        version: u16,
        node_id: XrNetPeerId,
        frame: XrNetStateFrame,
    },
    Alignment {
        version: u16,
        node_id: XrNetPeerId,
        frame: XrNetAlignmentFrame,
    },
    AlignmentDescriptor {
        version: u16,
        node_id: XrNetPeerId,
        frame: XrNetAlignmentDescriptorFrame,
    },
    Leave(XrNetLeavePacket),
}

#[derive(Debug)]
enum XrNetUdpOutgoing {
    State(XrNetStateFrame),
    Break,
}

#[derive(Debug)]
enum XrNetSyncOutgoing {
    PeerUp {
        peer: XrNetPeer,
        sync_addr: SocketAddr,
    },
    PeerRemoved {
        peer_id: XrNetPeerId,
    },
    Alignment(XrNetAlignmentFrame),
    AlignmentDescriptor(XrNetAlignmentDescriptorFrame),
    Break,
}

struct UdpWorkerPeerState {
    peer: XrNetPeer,
    sync_addr: SocketAddr,
    last_seen: Instant,
    last_state_seq: Option<u32>,
    last_alignment_seq: Option<u32>,
    last_alignment_descriptor_seq: Option<u32>,
}

impl UdpWorkerPeerState {
    fn new(peer: XrNetPeer, sync_addr: SocketAddr, now: Instant) -> Self {
        Self {
            peer,
            sync_addr,
            last_seen: now,
            last_state_seq: None,
            last_alignment_seq: None,
            last_alignment_descriptor_seq: None,
        }
    }
}

struct SyncWorkerPeerState {
    peer: XrNetPeer,
    sync_addr: SocketAddr,
    last_state_seq: Option<u32>,
    last_alignment_seq: Option<u32>,
    last_alignment_descriptor_seq: Option<u32>,
    sync_connection: Option<XrNetSyncConnection>,
    next_sync_connect_attempt_at: Instant,
}

impl SyncWorkerPeerState {
    fn new(peer: XrNetPeer, sync_addr: SocketAddr, now: Instant) -> Self {
        Self {
            peer,
            sync_addr,
            last_state_seq: None,
            last_alignment_seq: None,
            last_alignment_descriptor_seq: None,
            sync_connection: None,
            next_sync_connect_attempt_at: now,
        }
    }
}

struct XrNetSyncConnection {
    stream: TcpStream,
    peer_id: Option<XrNetPeerId>,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    handshake_received: bool,
}

impl XrNetSyncConnection {
    fn new(stream: TcpStream, peer_id: Option<XrNetPeerId>) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        let _ = stream.set_nodelay(true);
        Ok(Self {
            stream,
            peer_id,
            read_buf: Vec::new(),
            write_buf: Vec::new(),
            handshake_received: false,
        })
    }

    fn queue_packet(&mut self, packet: &XrNetSyncPacket) {
        let payload = packet.serialize_bin();
        if payload.len() > XR_NET_SYNC_MAX_FRAME_BYTES {
            return;
        }
        let frame_len = payload.len().min(u32::MAX as usize) as u32;
        self.write_buf.extend_from_slice(&frame_len.to_le_bytes());
        self.write_buf
            .extend_from_slice(&payload[..frame_len as usize]);
    }

    fn queue_hello(&mut self, node_id: XrNetPeerId) {
        self.queue_packet(&XrNetSyncPacket::Hello(XrNetSyncHello {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
        }));
    }

    fn shutdown(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

#[derive(Debug)]
pub struct XrNetNode {
    pub incoming_receiver: mpsc::Receiver<XrNetIncoming>,
    udp_outgoing_sender: mpsc::Sender<XrNetUdpOutgoing>,
    sync_outgoing_sender: mpsc::Sender<XrNetSyncOutgoing>,
    thread_loop: Arc<AtomicBool>,
    udp_worker_thread: Option<JoinHandle<()>>,
    sync_worker_thread: Option<JoinHandle<()>>,
    next_state_seq: u32,
    next_alignment_seq: u32,
    next_alignment_descriptor_seq: u32,
}

impl XrNetNode {
    pub fn new() -> io::Result<Self> {
        Self::with_config(XrNetConfig::default())
    }

    pub fn with_config(config: XrNetConfig) -> io::Result<Self> {
        let discovery_socket = UdpSocket::bind(config.discovery_bind)?;
        discovery_socket.set_nonblocking(true)?;
        discovery_socket.set_broadcast(true)?;

        let data_socket = UdpSocket::bind(config.data_bind)?;
        data_socket.set_nonblocking(true)?;

        let sync_listener = TcpListener::bind(config.sync_bind)?;
        sync_listener.set_nonblocking(true)?;

        let bound_data_addr = data_socket.local_addr()?;
        let bound_sync_addr = sync_listener.local_addr()?;
        let (incoming_sender, incoming_receiver) = mpsc::channel();
        let (udp_outgoing_sender, udp_outgoing_receiver) = mpsc::channel();
        let (sync_outgoing_sender, sync_outgoing_receiver) = mpsc::channel();
        let thread_loop = Arc::new(AtomicBool::new(true));
        let udp_worker_thread = Some(spawn_udp_worker_thread(
            config.node_id,
            config.discovery_targets,
            config.discovery_interval,
            config.peer_timeout,
            config.poll_interval,
            bound_data_addr.port(),
            bound_sync_addr.port(),
            discovery_socket,
            data_socket,
            incoming_sender.clone(),
            udp_outgoing_receiver,
            sync_outgoing_sender.clone(),
            thread_loop.clone(),
        ));
        let sync_worker_thread = Some(spawn_sync_worker_thread(
            config.node_id,
            config.poll_interval,
            sync_listener,
            incoming_sender,
            sync_outgoing_receiver,
            thread_loop.clone(),
        ));

        Ok(Self {
            incoming_receiver,
            udp_outgoing_sender,
            sync_outgoing_sender,
            thread_loop,
            udp_worker_thread,
            sync_worker_thread,
            next_state_seq: 0,
            next_alignment_seq: 0,
            next_alignment_descriptor_seq: 0,
        })
    }

    pub fn send_state(&mut self, state: XrState) {
        let frame = XrNetStateFrame {
            seq: self.next_state_seq,
            sent_at: state.time,
            state,
        };
        self.next_state_seq = self.next_state_seq.wrapping_add(1);
        let _ = self.udp_outgoing_sender.send(XrNetUdpOutgoing::State(frame));
    }

    pub fn send_alignment(&mut self, anchor: XrAnchor, confidence: f32, sent_at: f64) {
        let frame = XrNetAlignmentFrame {
            seq: self.next_alignment_seq,
            sent_at,
            anchor,
            confidence,
        };
        self.next_alignment_seq = self.next_alignment_seq.wrapping_add(1);
        let _ = self
            .sync_outgoing_sender
            .send(XrNetSyncOutgoing::Alignment(frame));
    }

    pub fn send_alignment_descriptor(&mut self, mut frame: XrNetAlignmentDescriptorFrame) {
        frame.seq = self.next_alignment_descriptor_seq;
        self.next_alignment_descriptor_seq = self.next_alignment_descriptor_seq.wrapping_add(1);
        let _ = self
            .sync_outgoing_sender
            .send(XrNetSyncOutgoing::AlignmentDescriptor(frame));
    }
}

impl Drop for XrNetNode {
    fn drop(&mut self) {
        self.thread_loop.store(false, Ordering::Relaxed);
        let _ = self.udp_outgoing_sender.send(XrNetUdpOutgoing::Break);
        let _ = self.sync_outgoing_sender.send(XrNetSyncOutgoing::Break);
        if let Some(worker_thread) = self.udp_worker_thread.take() {
            let _ = worker_thread.join();
        }
        if let Some(worker_thread) = self.sync_worker_thread.take() {
            let _ = worker_thread.join();
        }
    }
}

fn spawn_udp_worker_thread(
    node_id: XrNetPeerId,
    discovery_targets: Vec<SocketAddr>,
    discovery_interval: Duration,
    peer_timeout: Duration,
    poll_interval: Duration,
    bound_data_port: u16,
    bound_sync_port: u16,
    discovery_socket: UdpSocket,
    data_socket: UdpSocket,
    incoming_sender: mpsc::Sender<XrNetIncoming>,
    outgoing_receiver: mpsc::Receiver<XrNetUdpOutgoing>,
    sync_outgoing_sender: mpsc::Sender<XrNetSyncOutgoing>,
    thread_loop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut peers = HashMap::<XrNetPeerId, UdpWorkerPeerState>::new();
        let mut cached_state = None;
        let mut next_discovery_at = Instant::now();
        let mut read_buf = [0u8; 65536];

        while thread_loop.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now >= next_discovery_at {
                send_discovery_hello(
                    &discovery_socket,
                    &discovery_targets,
                    node_id,
                    bound_data_port,
                    bound_sync_port,
                );
                next_discovery_at = now + discovery_interval;
            }

            let should_break = drain_udp_outgoing_messages(
                &data_socket,
                &mut peers,
                node_id,
                &outgoing_receiver,
                &mut cached_state,
            );
            process_data_packets(
                &data_socket,
                &mut read_buf,
                &mut peers,
                &incoming_sender,
                &sync_outgoing_sender,
                node_id,
            );
            process_discovery_packets(
                &discovery_socket,
                &data_socket,
                &mut read_buf,
                &mut peers,
                &incoming_sender,
                &sync_outgoing_sender,
                node_id,
                &cached_state,
            );
            expire_timed_out_peers(
                &mut peers,
                &incoming_sender,
                &sync_outgoing_sender,
                peer_timeout,
            );

            if should_break {
                break;
            }
            thread::sleep(poll_interval);
        }

        broadcast_explicit_leave(&data_socket, &peers, node_id);
        broadcast_discovery_leave(&discovery_socket, &discovery_targets, node_id);
    })
}

fn spawn_sync_worker_thread(
    node_id: XrNetPeerId,
    poll_interval: Duration,
    sync_listener: TcpListener,
    incoming_sender: mpsc::Sender<XrNetIncoming>,
    outgoing_receiver: mpsc::Receiver<XrNetSyncOutgoing>,
    thread_loop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut peers = HashMap::<XrNetPeerId, SyncWorkerPeerState>::new();
        let mut pending_sync_connections = Vec::<XrNetSyncConnection>::new();
        let mut cached_alignment = None;
        let mut cached_alignment_descriptor = None;

        while thread_loop.load(Ordering::Relaxed) {
            let should_break = drain_sync_outgoing_messages(
                &mut peers,
                node_id,
                &outgoing_receiver,
                &mut cached_alignment,
                &mut cached_alignment_descriptor,
            );
            accept_sync_connections(&sync_listener, &mut pending_sync_connections, node_id);
            ensure_outbound_sync_connections(&mut peers, node_id);
            process_pending_sync_connections(
                &mut pending_sync_connections,
                &mut peers,
                &incoming_sender,
                node_id,
                &cached_alignment,
                &cached_alignment_descriptor,
            );
            process_sync_connections(
                &mut peers,
                &incoming_sender,
                node_id,
                &cached_alignment,
                &cached_alignment_descriptor,
            );

            if should_break {
                break;
            }
            thread::sleep(poll_interval);
        }

        for peer_state in peers.values_mut() {
            close_sync_connection(peer_state);
        }
        for pending in &mut pending_sync_connections {
            pending.shutdown();
        }
    })
}

fn send_discovery_hello(
    socket: &UdpSocket,
    discovery_targets: &[SocketAddr],
    node_id: XrNetPeerId,
    data_port: u16,
    sync_port: u16,
) {
    let packet = XrNetDiscoveryPacket::Hello(XrNetDiscoveryHello {
        version: XR_NET_PROTOCOL_VERSION,
        node_id,
        data_port,
        sync_port,
    });
    let buf = packet.serialize_bin();
    for target in discovery_targets {
        let _ = socket.send_to(&buf, target);
    }
}

fn broadcast_discovery_leave(
    socket: &UdpSocket,
    discovery_targets: &[SocketAddr],
    node_id: XrNetPeerId,
) {
    let packet = XrNetDiscoveryPacket::Leave(XrNetLeavePacket {
        version: XR_NET_PROTOCOL_VERSION,
        node_id,
    });
    let buf = packet.serialize_bin();
    for target in discovery_targets {
        let _ = socket.send_to(&buf, target);
    }
}

fn broadcast_explicit_leave(
    socket: &UdpSocket,
    peers: &HashMap<XrNetPeerId, UdpWorkerPeerState>,
    node_id: XrNetPeerId,
) {
    let packet = XrNetDataPacket::Leave(XrNetLeavePacket {
        version: XR_NET_PROTOCOL_VERSION,
        node_id,
    });
    let buf = packet.serialize_bin();
    for peer in peers.values() {
        let _ = socket.send_to(&buf, peer.peer.addr);
    }
}

fn drain_udp_outgoing_messages(
    socket: &UdpSocket,
    peers: &mut HashMap<XrNetPeerId, UdpWorkerPeerState>,
    node_id: XrNetPeerId,
    outgoing_receiver: &mpsc::Receiver<XrNetUdpOutgoing>,
    cached_state: &mut Option<XrNetStateFrame>,
) -> bool {
    loop {
        match outgoing_receiver.try_recv() {
            Ok(XrNetUdpOutgoing::State(frame)) => {
                *cached_state = Some(frame.clone());
                let packet = XrNetDataPacket::State {
                    version: XR_NET_PROTOCOL_VERSION,
                    node_id,
                    frame,
                };
                let buf = packet.serialize_bin();
                for peer in peers.values() {
                    let _ = socket.send_to(&buf, peer.peer.addr);
                }
            }
            Ok(XrNetUdpOutgoing::Break) => return true,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => return true,
        }
    }
}

fn drain_sync_outgoing_messages(
    peers: &mut HashMap<XrNetPeerId, SyncWorkerPeerState>,
    node_id: XrNetPeerId,
    outgoing_receiver: &mpsc::Receiver<XrNetSyncOutgoing>,
    cached_alignment: &mut Option<XrNetAlignmentFrame>,
    cached_alignment_descriptor: &mut Option<XrNetAlignmentDescriptorFrame>,
) -> bool {
    loop {
        match outgoing_receiver.try_recv() {
            Ok(XrNetSyncOutgoing::PeerUp { peer, sync_addr }) => {
                register_sync_peer(peers, peer, sync_addr);
            }
            Ok(XrNetSyncOutgoing::PeerRemoved { peer_id }) => {
                remove_sync_peer(peers, peer_id);
            }
            Ok(XrNetSyncOutgoing::Alignment(frame)) => {
                *cached_alignment = Some(frame);
                let packet = XrNetDataPacket::Alignment {
                    version: XR_NET_PROTOCOL_VERSION,
                    node_id,
                    frame,
                };
                for peer_state in peers.values_mut() {
                    if let Some(connection) = peer_state.sync_connection.as_mut() {
                        connection.queue_packet(&XrNetSyncPacket::Data(packet.clone()));
                    }
                }
            }
            Ok(XrNetSyncOutgoing::AlignmentDescriptor(frame)) => {
                *cached_alignment_descriptor = Some(frame.clone());
                let packet = XrNetDataPacket::AlignmentDescriptor {
                    version: XR_NET_PROTOCOL_VERSION,
                    node_id,
                    frame,
                };
                for peer_state in peers.values_mut() {
                    if let Some(connection) = peer_state.sync_connection.as_mut() {
                        connection.queue_packet(&XrNetSyncPacket::Data(packet.clone()));
                    }
                }
            }
            Ok(XrNetSyncOutgoing::Break) => return true,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => return true,
        }
    }
}

fn process_discovery_packets(
    discovery_socket: &UdpSocket,
    data_socket: &UdpSocket,
    read_buf: &mut [u8],
    peers: &mut HashMap<XrNetPeerId, UdpWorkerPeerState>,
    incoming_sender: &mpsc::Sender<XrNetIncoming>,
    sync_outgoing_sender: &mpsc::Sender<XrNetSyncOutgoing>,
    node_id: XrNetPeerId,
    cached_state: &Option<XrNetStateFrame>,
) {
    loop {
        match discovery_socket.recv_from(read_buf) {
            Ok((len, source_addr)) => {
                let Ok(packet) = XrNetDiscoveryPacket::deserialize_bin(&read_buf[..len]) else {
                    continue;
                };
                match packet {
                    XrNetDiscoveryPacket::Hello(packet) => {
                        if packet.version != XR_NET_PROTOCOL_VERSION || packet.node_id == node_id {
                            continue;
                        }
                        let peer_addr = SocketAddr::new(source_addr.ip(), packet.data_port);
                        let peer_sync_addr = SocketAddr::new(source_addr.ip(), packet.sync_port);
                        let (peer, is_new) = touch_udp_peer(
                            peers,
                            incoming_sender,
                            sync_outgoing_sender,
                            packet.node_id,
                            peer_addr,
                            peer_sync_addr,
                        );
                        if is_new {
                            send_cached_state_to_peer(data_socket, peer, node_id, cached_state);
                        }
                    }
                    XrNetDiscoveryPacket::Leave(packet) => {
                        if packet.version != XR_NET_PROTOCOL_VERSION || packet.node_id == node_id {
                            continue;
                        }
                        remove_udp_peer(
                            peers,
                            incoming_sender,
                            sync_outgoing_sender,
                            packet.node_id,
                            XrNetLeaveReason::Explicit,
                        );
                    }
                }
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => return,
            Err(_) => return,
        }
    }
}

fn process_data_packets(
    socket: &UdpSocket,
    read_buf: &mut [u8],
    peers: &mut HashMap<XrNetPeerId, UdpWorkerPeerState>,
    incoming_sender: &mpsc::Sender<XrNetIncoming>,
    sync_outgoing_sender: &mpsc::Sender<XrNetSyncOutgoing>,
    node_id: XrNetPeerId,
) {
    loop {
        match socket.recv_from(read_buf) {
            Ok((len, source_addr)) => {
                let Ok(packet) = XrNetDataPacket::deserialize_bin(&read_buf[..len]) else {
                    continue;
                };
                match packet {
                    XrNetDataPacket::State {
                        version,
                        node_id: remote_id,
                        frame,
                    } => {
                        if version != XR_NET_PROTOCOL_VERSION || remote_id == node_id {
                            continue;
                        }
                        let sync_addr = peers
                            .get(&remote_id)
                            .map(|peer_state| peer_state.sync_addr)
                            .unwrap_or_else(|| {
                                SocketAddr::new(source_addr.ip(), XR_NET_DEFAULT_SYNC_PORT)
                            });
                        let (peer, _) =
                            touch_udp_peer(
                                peers,
                                incoming_sender,
                                sync_outgoing_sender,
                                remote_id,
                                source_addr,
                                sync_addr,
                            );
                        let Some(peer_state) = peers.get_mut(&remote_id) else {
                            continue;
                        };
                        if !accept_seq(peer_state.last_state_seq, frame.seq) {
                            continue;
                        }
                        peer_state.last_state_seq = Some(frame.seq);
                        let _ = incoming_sender.send(XrNetIncoming::State { peer, frame });
                    }
                    XrNetDataPacket::Alignment {
                        version,
                        node_id: remote_id,
                        frame,
                    } => {
                        if version != XR_NET_PROTOCOL_VERSION || remote_id == node_id {
                            continue;
                        }
                        let sync_addr = peers
                            .get(&remote_id)
                            .map(|peer_state| peer_state.sync_addr)
                            .unwrap_or_else(|| {
                                SocketAddr::new(source_addr.ip(), XR_NET_DEFAULT_SYNC_PORT)
                            });
                        let (peer, _) =
                            touch_udp_peer(
                                peers,
                                incoming_sender,
                                sync_outgoing_sender,
                                remote_id,
                                source_addr,
                                sync_addr,
                            );
                        let Some(peer_state) = peers.get_mut(&remote_id) else {
                            continue;
                        };
                        if !accept_seq(peer_state.last_alignment_seq, frame.seq) {
                            continue;
                        }
                        peer_state.last_alignment_seq = Some(frame.seq);
                        let _ = incoming_sender.send(XrNetIncoming::Alignment { peer, frame });
                    }
                    XrNetDataPacket::AlignmentDescriptor {
                        version,
                        node_id: remote_id,
                        frame,
                    } => {
                        if version != XR_NET_PROTOCOL_VERSION || remote_id == node_id {
                            continue;
                        }
                        let sync_addr = peers
                            .get(&remote_id)
                            .map(|peer_state| peer_state.sync_addr)
                            .unwrap_or_else(|| {
                                SocketAddr::new(source_addr.ip(), XR_NET_DEFAULT_SYNC_PORT)
                            });
                        let (peer, _) =
                            touch_udp_peer(
                                peers,
                                incoming_sender,
                                sync_outgoing_sender,
                                remote_id,
                                source_addr,
                                sync_addr,
                            );
                        let Some(peer_state) = peers.get_mut(&remote_id) else {
                            continue;
                        };
                        if !accept_seq(peer_state.last_alignment_descriptor_seq, frame.seq) {
                            continue;
                        }
                        peer_state.last_alignment_descriptor_seq = Some(frame.seq);
                        let _ = incoming_sender
                            .send(XrNetIncoming::AlignmentDescriptor { peer, frame });
                    }
                    XrNetDataPacket::Leave(packet) => {
                        if packet.version != XR_NET_PROTOCOL_VERSION || packet.node_id == node_id {
                            continue;
                        }
                        remove_udp_peer(
                            peers,
                            incoming_sender,
                            sync_outgoing_sender,
                            packet.node_id,
                            XrNetLeaveReason::Explicit,
                        );
                    }
                }
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => return,
            Err(_) => return,
        }
    }
}

fn touch_udp_peer(
    peers: &mut HashMap<XrNetPeerId, UdpWorkerPeerState>,
    incoming_sender: &mpsc::Sender<XrNetIncoming>,
    sync_outgoing_sender: &mpsc::Sender<XrNetSyncOutgoing>,
    peer_id: XrNetPeerId,
    addr: SocketAddr,
    sync_addr: SocketAddr,
) -> (XrNetPeer, bool) {
    let now = Instant::now();
    if let Some(peer_state) = peers.get_mut(&peer_id) {
        let peer_changed = peer_state.peer.addr != addr || peer_state.sync_addr != sync_addr;
        peer_state.peer.addr = addr;
        peer_state.sync_addr = sync_addr;
        peer_state.last_seen = now;
        if peer_changed {
            let _ = sync_outgoing_sender.send(XrNetSyncOutgoing::PeerUp {
                peer: peer_state.peer,
                sync_addr,
            });
        }
        (peer_state.peer, false)
    } else {
        let peer = XrNetPeer { id: peer_id, addr };
        peers.insert(peer_id, UdpWorkerPeerState::new(peer, sync_addr, now));
        let _ = incoming_sender.send(XrNetIncoming::Join { peer });
        let _ = sync_outgoing_sender.send(XrNetSyncOutgoing::PeerUp { peer, sync_addr });
        (peer, true)
    }
}

fn send_cached_state_to_peer(
    socket: &UdpSocket,
    peer: XrNetPeer,
    node_id: XrNetPeerId,
    cached_state: &Option<XrNetStateFrame>,
) {
    if let Some(frame) = cached_state {
        let packet = XrNetDataPacket::State {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
            frame: frame.clone(),
        };
        let _ = socket.send_to(&packet.serialize_bin(), peer.addr);
    }
}

fn close_sync_connection(peer_state: &mut SyncWorkerPeerState) {
    if let Some(connection) = peer_state.sync_connection.as_mut() {
        connection.shutdown();
    }
    peer_state.sync_connection = None;
}

fn register_sync_peer(
    peers: &mut HashMap<XrNetPeerId, SyncWorkerPeerState>,
    peer: XrNetPeer,
    sync_addr: SocketAddr,
) {
    let now = Instant::now();
    if let Some(peer_state) = peers.get_mut(&peer.id) {
        let sync_addr_changed = peer_state.sync_addr != sync_addr;
        peer_state.peer = peer;
        peer_state.sync_addr = sync_addr;
        if sync_addr_changed {
            close_sync_connection(peer_state);
            peer_state.next_sync_connect_attempt_at = now;
        }
    } else {
        peers.insert(peer.id, SyncWorkerPeerState::new(peer, sync_addr, now));
    }
}

fn remove_sync_peer(peers: &mut HashMap<XrNetPeerId, SyncWorkerPeerState>, peer_id: XrNetPeerId) {
    let Some(mut peer_state) = peers.remove(&peer_id) else {
        return;
    };
    close_sync_connection(&mut peer_state);
}

fn should_initiate_sync_connection(local_node_id: XrNetPeerId, peer_id: XrNetPeerId) -> bool {
    local_node_id.0 < peer_id.0
}

fn accept_sync_connections(
    sync_listener: &TcpListener,
    pending_sync_connections: &mut Vec<XrNetSyncConnection>,
    node_id: XrNetPeerId,
) {
    loop {
        match sync_listener.accept() {
            Ok((stream, _)) => {
                let Ok(mut connection) = XrNetSyncConnection::new(stream, None) else {
                    continue;
                };
                connection.queue_hello(node_id);
                pending_sync_connections.push(connection);
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => return,
            Err(_) => return,
        }
    }
}

fn ensure_outbound_sync_connections(
    peers: &mut HashMap<XrNetPeerId, SyncWorkerPeerState>,
    node_id: XrNetPeerId,
) {
    let now = Instant::now();
    let peer_ids = peers.keys().copied().collect::<Vec<_>>();
    for peer_id in peer_ids {
        let Some(peer_state) = peers.get_mut(&peer_id) else {
            continue;
        };
        if peer_state.sync_connection.is_some()
            || !should_initiate_sync_connection(node_id, peer_id)
            || now < peer_state.next_sync_connect_attempt_at
        {
            continue;
        }
        peer_state.next_sync_connect_attempt_at = now + XR_NET_DEFAULT_SYNC_CONNECT_RETRY;
        let Ok(stream) =
            TcpStream::connect_timeout(&peer_state.sync_addr, XR_NET_SYNC_CONNECT_TIMEOUT)
        else {
            continue;
        };
        let Ok(mut connection) = XrNetSyncConnection::new(stream, Some(peer_id)) else {
            continue;
        };
        connection.queue_hello(node_id);
        peer_state.sync_connection = Some(connection);
    }
}

fn process_pending_sync_connections(
    pending_sync_connections: &mut Vec<XrNetSyncConnection>,
    peers: &mut HashMap<XrNetPeerId, SyncWorkerPeerState>,
    incoming_sender: &mpsc::Sender<XrNetIncoming>,
    node_id: XrNetPeerId,
    cached_alignment: &Option<XrNetAlignmentFrame>,
    cached_alignment_descriptor: &Option<XrNetAlignmentDescriptorFrame>,
) {
    let pending_count = pending_sync_connections.len();
    for _ in 0..pending_count {
        let mut connection = pending_sync_connections.swap_remove(0);
        let Ok(packets) = pump_sync_connection(&mut connection) else {
            connection.shutdown();
            continue;
        };
        let mut queued_data = Vec::<XrNetDataPacket>::new();
        let mut invalid = false;

        for packet in packets {
            match packet {
                XrNetSyncPacket::Hello(hello) => {
                    if hello.version != XR_NET_PROTOCOL_VERSION || hello.node_id == node_id {
                        invalid = true;
                        break;
                    }
                    if connection
                        .peer_id
                        .is_some_and(|existing| existing != hello.node_id)
                    {
                        invalid = true;
                        break;
                    }
                    connection.peer_id = Some(hello.node_id);
                    connection.handshake_received = true;
                }
                XrNetSyncPacket::Data(data) => {
                    if !connection.handshake_received {
                        invalid = true;
                        break;
                    }
                    queued_data.push(data);
                }
            }
        }

        if invalid {
            connection.shutdown();
            continue;
        }

        let Some(peer_id) = connection.peer_id else {
            pending_sync_connections.push(connection);
            continue;
        };
        let Some(peer_state) = peers.get_mut(&peer_id) else {
            pending_sync_connections.push(connection);
            continue;
        };

        close_sync_connection(peer_state);
        connection.handshake_received = true;
        send_cached_sync_frames_to_connection(
            &mut connection,
            node_id,
            cached_alignment,
            cached_alignment_descriptor,
        );
        peer_state.sync_connection = Some(connection);

        if let Some(peer_state) = peers.get_mut(&peer_id) {
            for data in queued_data {
                route_sync_data_packet(peer_state, incoming_sender, node_id, data);
            }
        }
    }
}

fn process_sync_connections(
    peers: &mut HashMap<XrNetPeerId, SyncWorkerPeerState>,
    incoming_sender: &mpsc::Sender<XrNetIncoming>,
    node_id: XrNetPeerId,
    cached_alignment: &Option<XrNetAlignmentFrame>,
    cached_alignment_descriptor: &Option<XrNetAlignmentDescriptorFrame>,
) {
    let peer_ids = peers.keys().copied().collect::<Vec<_>>();
    for peer_id in peer_ids {
        let Some(mut connection) = peers
            .get_mut(&peer_id)
            .and_then(|peer_state| peer_state.sync_connection.take())
        else {
            continue;
        };

        let result = pump_sync_connection(&mut connection);
        let Ok(packets) = result else {
            connection.shutdown();
            continue;
        };

        let mut invalid = false;
        let mut became_ready = false;
        let mut queued_data = Vec::<XrNetDataPacket>::new();
        for packet in packets {
            match packet {
                XrNetSyncPacket::Hello(hello) => {
                    if hello.version != XR_NET_PROTOCOL_VERSION || hello.node_id != peer_id {
                        invalid = true;
                        break;
                    }
                    if !connection.handshake_received {
                        connection.handshake_received = true;
                        became_ready = true;
                    }
                }
                XrNetSyncPacket::Data(data) => {
                    if !connection.handshake_received {
                        invalid = true;
                        break;
                    }
                    queued_data.push(data);
                }
            }
        }

        let Some(peer_state) = peers.get_mut(&peer_id) else {
            connection.shutdown();
            continue;
        };
        if invalid {
            connection.shutdown();
            peer_state.next_sync_connect_attempt_at =
                Instant::now() + XR_NET_DEFAULT_SYNC_CONNECT_RETRY;
            continue;
        }
        if became_ready {
            send_cached_sync_frames_to_connection(
                &mut connection,
                node_id,
                cached_alignment,
                cached_alignment_descriptor,
            );
        }
        for data in queued_data {
            route_sync_data_packet(peer_state, incoming_sender, node_id, data);
        }
        peer_state.sync_connection = Some(connection);
    }
}

fn send_cached_sync_frames_to_connection(
    connection: &mut XrNetSyncConnection,
    node_id: XrNetPeerId,
    cached_alignment: &Option<XrNetAlignmentFrame>,
    cached_alignment_descriptor: &Option<XrNetAlignmentDescriptorFrame>,
) {
    if let Some(frame) = cached_alignment {
        connection.queue_packet(&XrNetSyncPacket::Data(XrNetDataPacket::Alignment {
            version: XR_NET_PROTOCOL_VERSION,
            node_id,
            frame: *frame,
        }));
    }
    if let Some(frame) = cached_alignment_descriptor {
        connection.queue_packet(&XrNetSyncPacket::Data(
            XrNetDataPacket::AlignmentDescriptor {
                version: XR_NET_PROTOCOL_VERSION,
                node_id,
                frame: frame.clone(),
            },
        ));
    }
}

fn pump_sync_connection(connection: &mut XrNetSyncConnection) -> io::Result<Vec<XrNetSyncPacket>> {
    flush_sync_connection(connection)?;

    let mut read_chunk = [0u8; 16384];
    let mut total_read = 0usize;
    loop {
        match connection.stream.read(&mut read_chunk) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "sync connection closed",
                ))
            }
            Ok(len) => {
                connection.read_buf.extend_from_slice(&read_chunk[..len]);
                total_read += len;
                if total_read >= XR_NET_SYNC_READ_BUDGET_BYTES_PER_POLL {
                    break;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
            Err(err) => return Err(err),
        }
    }
    drain_sync_packets(&mut connection.read_buf)
}

fn flush_sync_connection(connection: &mut XrNetSyncConnection) -> io::Result<()> {
    let mut total_written = 0usize;
    while !connection.write_buf.is_empty() {
        match connection.stream.write(&connection.write_buf) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "sync connection write zero",
                ))
            }
            Ok(written) => {
                consume_vec_prefix(&mut connection.write_buf, written);
                total_written += written;
                if total_written >= XR_NET_SYNC_WRITE_BUDGET_BYTES_PER_POLL {
                    break;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn drain_sync_packets(read_buf: &mut Vec<u8>) -> io::Result<Vec<XrNetSyncPacket>> {
    let mut packets = Vec::<XrNetSyncPacket>::new();
    let mut offset = 0usize;
    while read_buf.len().saturating_sub(offset) >= 4 {
        let frame_len = u32::from_le_bytes([
            read_buf[offset],
            read_buf[offset + 1],
            read_buf[offset + 2],
            read_buf[offset + 3],
        ]) as usize;
        if frame_len > XR_NET_SYNC_MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "sync frame exceeded max size",
            ));
        }
        if read_buf.len().saturating_sub(offset + 4) < frame_len {
            break;
        }
        let start = offset + 4;
        let end = start + frame_len;
        let packet = XrNetSyncPacket::deserialize_bin(&read_buf[start..end]).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "failed to decode sync packet")
        })?;
        packets.push(packet);
        offset = end;
    }
    if offset > 0 {
        consume_vec_prefix(read_buf, offset);
    }
    Ok(packets)
}

fn consume_vec_prefix(buf: &mut Vec<u8>, len: usize) {
    if len == 0 {
        return;
    }
    if len >= buf.len() {
        buf.clear();
        return;
    }
    let remaining = buf.len() - len;
    buf.copy_within(len.., 0);
    buf.truncate(remaining);
}

fn route_sync_data_packet(
    peer_state: &mut SyncWorkerPeerState,
    incoming_sender: &mpsc::Sender<XrNetIncoming>,
    node_id: XrNetPeerId,
    packet: XrNetDataPacket,
) {
    let peer = peer_state.peer;
    match packet {
        XrNetDataPacket::State {
            version,
            node_id: remote_id,
            frame,
        } => {
            if version != XR_NET_PROTOCOL_VERSION || remote_id == node_id {
                return;
            }
            if !accept_seq(peer_state.last_state_seq, frame.seq) {
                return;
            }
            peer_state.last_state_seq = Some(frame.seq);
            let _ = incoming_sender.send(XrNetIncoming::State { peer, frame });
        }
        XrNetDataPacket::Alignment {
            version,
            node_id: remote_id,
            frame,
        } => {
            if version != XR_NET_PROTOCOL_VERSION || remote_id == node_id {
                return;
            }
            if !accept_seq(peer_state.last_alignment_seq, frame.seq) {
                return;
            }
            peer_state.last_alignment_seq = Some(frame.seq);
            let _ = incoming_sender.send(XrNetIncoming::Alignment { peer, frame });
        }
        XrNetDataPacket::AlignmentDescriptor {
            version,
            node_id: remote_id,
            frame,
        } => {
            if version != XR_NET_PROTOCOL_VERSION || remote_id == node_id {
                return;
            }
            if !accept_seq(peer_state.last_alignment_descriptor_seq, frame.seq) {
                return;
            }
            peer_state.last_alignment_descriptor_seq = Some(frame.seq);
            let _ = incoming_sender.send(XrNetIncoming::AlignmentDescriptor { peer, frame });
        }
        XrNetDataPacket::Leave(packet) => {
            if packet.version != XR_NET_PROTOCOL_VERSION || packet.node_id == node_id {
                return;
            }
        }
    }
}

pub fn encode_alignment_descriptor_packet(
    node_id: XrNetPeerId,
    frame: &XrNetAlignmentDescriptorFrame,
) -> Vec<u8> {
    XrNetDataPacket::AlignmentDescriptor {
        version: XR_NET_PROTOCOL_VERSION,
        node_id,
        frame: frame.clone(),
    }
    .serialize_bin()
}

pub fn decode_alignment_descriptor_packet(
    bytes: &[u8],
) -> Option<(XrNetPeerId, XrNetAlignmentDescriptorFrame)> {
    match XrNetDataPacket::deserialize_bin(bytes).ok()? {
        XrNetDataPacket::AlignmentDescriptor {
            version,
            node_id,
            frame,
        } if version == XR_NET_PROTOCOL_VERSION => Some((node_id, frame)),
        _ => None,
    }
}

#[cfg(test)]
fn wrap_angle(mut angle: f32) -> f32 {
    while angle <= -std::f32::consts::PI {
        angle += std::f32::consts::TAU;
    }
    while angle > std::f32::consts::PI {
        angle -= std::f32::consts::TAU;
    }
    angle
}

fn remove_udp_peer(
    peers: &mut HashMap<XrNetPeerId, UdpWorkerPeerState>,
    incoming_sender: &mpsc::Sender<XrNetIncoming>,
    sync_outgoing_sender: &mpsc::Sender<XrNetSyncOutgoing>,
    peer_id: XrNetPeerId,
    reason: XrNetLeaveReason,
) {
    let Some(peer_state) = peers.remove(&peer_id) else {
        return;
    };
    let _ = sync_outgoing_sender.send(XrNetSyncOutgoing::PeerRemoved { peer_id });
    let _ = incoming_sender.send(XrNetIncoming::Leave {
        peer: peer_state.peer,
        reason,
    });
}

fn expire_timed_out_peers(
    peers: &mut HashMap<XrNetPeerId, UdpWorkerPeerState>,
    incoming_sender: &mpsc::Sender<XrNetIncoming>,
    sync_outgoing_sender: &mpsc::Sender<XrNetSyncOutgoing>,
    peer_timeout: Duration,
) {
    let now = Instant::now();
    let expired: Vec<_> = peers
        .iter()
        .filter_map(|(peer_id, peer_state)| {
            (now.duration_since(peer_state.last_seen) > peer_timeout).then_some(*peer_id)
        })
        .collect();
    for peer_id in expired {
        remove_udp_peer(
            peers,
            incoming_sender,
            sync_outgoing_sender,
            peer_id,
            XrNetLeaveReason::Timeout,
        );
    }
}

fn accept_seq(last: Option<u32>, next: u32) -> bool {
    let Some(last) = last else {
        return true;
    };
    next != last && next.wrapping_sub(last) < (u32::MAX / 2)
}

fn default_node_id() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id() as u128;
    LiveId::from_str(&format!("xr-net-{}-{}", now, pid)).0
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_IO_TIMEOUT: Duration = Duration::from_secs(3);

    fn wait_for_event<F>(node: &XrNetNode, mut predicate: F) -> Option<XrNetIncoming>
    where
        F: FnMut(&XrNetIncoming) -> bool,
    {
        let start = Instant::now();
        while start.elapsed() < TEST_IO_TIMEOUT {
            match node
                .incoming_receiver
                .recv_timeout(Duration::from_millis(50))
            {
                Ok(event) if predicate(&event) => return Some(event),
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
        None
    }

    fn localhost_config(
        node_id: u64,
        discovery_port: u16,
        data_port: u16,
        sync_port: u16,
        discovery_targets: Vec<u16>,
    ) -> XrNetConfig {
        XrNetConfig {
            node_id: XrNetPeerId(node_id),
            discovery_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), discovery_port),
            data_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), data_port),
            sync_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), sync_port),
            discovery_targets: discovery_targets
                .into_iter()
                .map(|port| SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
                .collect(),
            discovery_interval: Duration::from_millis(20),
            peer_timeout: Duration::from_millis(150),
            poll_interval: Duration::from_millis(5),
        }
    }

    fn make_test_state(head_position: Vec3f) -> XrState {
        XrState {
            time: 1.0,
            head_pose: Pose::new(Quat::default(), head_position),
            ..XrState::default()
        }
    }

    fn transform_point(mat: &Mat4f, point: Vec3f) -> Vec3f {
        mat.transform_vec4(vec4f(point.x, point.y, point.z, 1.0))
            .to_vec3f()
    }

    fn angle_error(a: f32, b: f32) -> f32 {
        wrap_angle(a - b).abs()
    }

    fn synthetic_scene_height(point: Vec2f) -> f32 {
        let mut height: f32 = 0.02;
        if point.x.abs() >= 2.05 && point.x.abs() <= 2.25 && point.y >= -2.30 && point.y <= 2.10 {
            height = height.max(2.15);
        }
        if point.y >= 1.75 && point.y <= 1.95 && point.x >= -2.25 && point.x <= 2.25 {
            height = height.max(2.15);
        }
        if point.y <= -1.95 && point.y >= -2.20 && point.x >= -2.25 && point.x <= 2.05 {
            height = height.max(2.15);
        }
        if point.x >= -0.95 && point.x <= -0.10 && point.y >= -0.42 && point.y <= 0.36 {
            height = height.max(0.84);
        }
        if point.x >= 0.52 && point.x <= 1.12 && point.y >= 0.52 && point.y <= 1.18 {
            height = height.max(1.38);
        }
        if point.x >= -1.58 && point.x <= -1.20 && point.y >= 0.92 && point.y <= 1.34 {
            height = height.max(1.62);
        }
        if point.x >= 0.96 && point.x <= 1.34 && point.y >= -1.42 && point.y <= -0.66 {
            height = height.max(0.68);
        }
        let wobble = ((point.x * 1.13 + point.y * 0.73).sin() * 0.018)
            + ((point.x * 0.41 - point.y * 1.51).cos() * 0.014);
        (height + wobble).clamp(0.0, 2.25)
    }

    fn make_descriptor_frame() -> XrNetAlignmentDescriptorFrame {
        let size = 120usize;
        let cell_size_meters = 0.05;
        let extent = size as f32 * cell_size_meters;
        let origin = -extent * 0.5;
        let bottom_y_meters = 0.0;
        let top_y_meters = 2.3;
        let floor_y_meters = 0.0;
        let mut heights_meters = vec![f32::NAN; size * size];
        for z in 0..size {
            for x in 0..size {
                let point = vec2f(
                    origin + (x as f32 + 0.5) * cell_size_meters,
                    origin + (z as f32 + 0.5) * cell_size_meters,
                );
                heights_meters[x + z * size] = synthetic_scene_height(point);
            }
        }
        XrNetAlignmentDescriptorFrame {
            seq: 0,
            sent_at: 1.0,
            descriptor: XrDepthAlignDescriptor {
                voxel_size_meters: 0.05,
                floor_y: floor_y_meters,
                wall_normal_histogram: Vec::new(),
                samples: Vec::new(),
                vertical_descriptor: None,
                height_map: Some(XrDepthAlignHeightMap {
                    origin_x: origin,
                    origin_z: origin,
                    cell_size_meters,
                    size_x: size as u16,
                    size_z: size as u16,
                    bottom_y_meters,
                    top_y_meters,
                    floor_y_meters,
                    player_cutout_center: None,
                    player_cutout_radius_meters: 0.0,
                    heights_meters,
                }),
            },
        }
    }

    fn make_tsdf_snapshot_for_descriptor(
        descriptor: XrDepthAlignDescriptor,
    ) -> TsdfPublishedSnapshot {
        TsdfPublishedSnapshot {
            generation: 1,
            latest_topology_generation: 1,
            update_sequence: 1,
            grid: Arc::new(SparseTsdGridReadSnapshot {
                voxel_size: descriptor.voxel_size_meters,
                ..Default::default()
            }),
            height_map: descriptor.height_map.clone(),
        }
    }

    #[test]
    fn peer_timeout_emits_leave_without_needing_more_packets() {
        let config = localhost_config(1, 42646, 42647, 42648, Vec::new());
        let node = XrNetNode::with_config(config).expect("node should bind localhost test ports");

        let discovery_socket =
            UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).unwrap();
        let data_socket =
            UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).unwrap();

        let discovery_packet = XrNetDiscoveryPacket::Hello(XrNetDiscoveryHello {
            version: XR_NET_PROTOCOL_VERSION,
            node_id: XrNetPeerId(2),
            data_port: data_socket.local_addr().unwrap().port(),
            sync_port: 42658,
        });
        let discovery_target = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 42646);
        let _ = discovery_socket.send_to(&discovery_packet.serialize_bin(), discovery_target);

        let join = wait_for_event(&node, |event| matches!(event, XrNetIncoming::Join { .. }))
            .expect("join should arrive from a single hello");
        let peer = match join {
            XrNetIncoming::Join { peer } => peer,
            _ => unreachable!(),
        };

        let state_packet = XrNetDataPacket::State {
            version: XR_NET_PROTOCOL_VERSION,
            node_id: XrNetPeerId(2),
            frame: XrNetStateFrame {
                seq: 0,
                sent_at: 1.0,
                state: make_test_state(vec3f(0.0, 1.6, 0.0)),
            },
        };
        let state_target = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 42647);
        let _ = data_socket.send_to(&state_packet.serialize_bin(), state_target);

        let _ = wait_for_event(&node, |event| {
            matches!(event, XrNetIncoming::State { peer: event_peer, .. } if *event_peer == peer)
        })
        .expect("state should arrive");

        let leave = wait_for_event(&node, |event| {
            matches!(
                event,
                XrNetIncoming::Leave {
                    peer: event_peer,
                    reason: XrNetLeaveReason::Timeout,
                } if *event_peer == peer
            )
        })
        .expect("peer should time out without any extra packets");
        match leave {
            XrNetIncoming::Leave { reason, .. } => {
                assert_eq!(reason, XrNetLeaveReason::Timeout);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn two_test_clients_align_two_cubes_via_protocol() {
        let mut left =
            XrNetNode::with_config(localhost_config(11, 42746, 42747, 42748, vec![42756]))
                .expect("left test client should bind");
        let mut right =
            XrNetNode::with_config(localhost_config(22, 42756, 42757, 42758, vec![42746]))
                .expect("right test client should bind");

        let left_cube_world = vec3f(-0.25, 0.82, -0.62);
        let right_cube_world = vec3f(0.21, 0.82, -0.38);
        let local_anchor = XrAnchor {
            left: left_cube_world,
            right: right_cube_world,
        };

        let remote_scene_to_local = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), 0.42),
            vec3f(-0.34, 0.05, 0.27),
        )
        .to_mat4();
        let local_to_remote_scene = remote_scene_to_local.invert();
        let remote_anchor = XrAnchor {
            left: transform_point(&local_to_remote_scene, left_cube_world),
            right: transform_point(&local_to_remote_scene, right_cube_world),
        };

        left.send_alignment(local_anchor, 0.95, 1.0);
        right.send_alignment(remote_anchor, 0.95, 1.0);

        let left_remote = wait_for_event(&left, |event| {
            matches!(event, XrNetIncoming::Alignment { .. })
        })
        .expect("left test client should receive remote alignment");
        let right_remote = wait_for_event(&right, |event| {
            matches!(event, XrNetIncoming::Alignment { .. })
        })
        .expect("right test client should receive remote alignment");

        let left_remote_frame = match left_remote {
            XrNetIncoming::Alignment { frame, .. } => frame,
            _ => unreachable!(),
        };
        let right_remote_frame = match right_remote {
            XrNetIncoming::Alignment { frame, .. } => frame,
            _ => unreachable!(),
        };

        let solved_remote_to_local = XrNetAlignmentFrame::remote_to_local_transform(
            &local_anchor_frame(local_anchor),
            &left_remote_frame,
        )
        .expect("left side should solve a remote->local transform");
        let solved_local_to_remote = XrNetAlignmentFrame::remote_to_local_transform(
            &remote_anchor_frame(remote_anchor),
            &right_remote_frame,
        )
        .expect("right side should solve a local->remote transform");

        let mapped_left = transform_point(&solved_remote_to_local, remote_anchor.left);
        let mapped_right = transform_point(&solved_remote_to_local, remote_anchor.right);
        assert!((mapped_left - left_cube_world).length() < 0.02);
        assert!((mapped_right - right_cube_world).length() < 0.02);

        let mapped_back_left = transform_point(&solved_local_to_remote, left_cube_world);
        let mapped_back_right = transform_point(&solved_local_to_remote, right_cube_world);
        assert!((mapped_back_left - remote_anchor.left).length() < 0.02);
        assert!((mapped_back_right - remote_anchor.right).length() < 0.02);
    }

    #[test]
    fn cached_alignment_descriptor_is_sent_to_late_joiner() {
        let mut left =
            XrNetNode::with_config(localhost_config(101, 42846, 42847, 42848, vec![42856]))
                .expect("left test client should bind");
        let right = XrNetNode::with_config(localhost_config(202, 42856, 42857, 42858, vec![42846]))
            .expect("right test client should bind");

        let descriptor = make_descriptor_frame();
        left.send_alignment_descriptor(descriptor.clone());

        let event = wait_for_event(&right, |event| {
            matches!(event, XrNetIncoming::AlignmentDescriptor { .. })
        })
        .expect("late joiner should receive cached descriptor");

        let received = match event {
            XrNetIncoming::AlignmentDescriptor { frame, .. } => frame,
            _ => unreachable!(),
        };
        assert_eq!(received.descriptor, descriptor.descriptor);
    }

    #[test]
    fn descriptor_solver_recovers_yaw_and_position_from_walls_and_vertical_descriptor() {
        let local = make_descriptor_frame();
        let ground_truth_remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), 0.58),
            vec3f(-0.82, 0.0, 0.67),
        )
        .to_mat4();
        let local_to_remote = ground_truth_remote_to_local.invert();
        let remote = local.transformed(&local_to_remote);

        let solution = XrNetAlignmentDescriptorFrame::solve_remote_to_local(&local, &remote)
            .expect("descriptor solver should find a transform");

        assert!(angle_error(solution.yaw_radians, 0.58) < 0.08);
        assert!((solution.translation - vec3f(-0.82, 0.0, 0.67)).length() < 0.12);
        assert!(solution.confidence > 0.15);
        assert!(solution.matched_samples >= 2);
    }

    #[test]
    fn descriptor_from_tsdf_snapshot_uses_tsdf_alignment_descriptor_snapshot() {
        let descriptor = make_descriptor_frame().descriptor;
        let snapshot = make_tsdf_snapshot_for_descriptor(descriptor.clone());
        let descriptor = XrNetAlignmentDescriptorFrame::from_tsdf_snapshot(&snapshot, 1.0)
            .expect("descriptor should use the TSDF alignment descriptor from the depth snapshot");

        assert_eq!(descriptor.descriptor.height_map, snapshot.height_map);
        assert!(descriptor.test_markers().is_some());
    }

    fn local_anchor_frame(anchor: XrAnchor) -> XrNetAlignmentFrame {
        XrNetAlignmentFrame {
            seq: 0,
            sent_at: 1.0,
            anchor,
            confidence: 1.0,
        }
    }

    fn remote_anchor_frame(anchor: XrAnchor) -> XrNetAlignmentFrame {
        XrNetAlignmentFrame {
            seq: 0,
            sent_at: 1.0,
            anchor,
            confidence: 1.0,
        }
    }
}
