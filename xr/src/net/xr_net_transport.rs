use super::*;
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(super) enum XrNetUdpOutgoing {
    State(XrNetStateFrame),
    Break,
}

#[derive(Debug)]
pub(super) enum XrNetSyncOutgoing {
    PeerUp {
        peer: XrNetPeer,
        sync_addr: SocketAddr,
    },
    PeerRemoved {
        peer_id: XrNetPeerId,
    },
    Alignment(XrNetAlignmentFrame),
    AlignmentDescriptor(XrNetAlignmentDescriptorFrame),
    Activity(XrNetActivityControl),
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

    fn close_connection(&mut self) {
        if let Some(connection) = self.sync_connection.as_mut() {
            connection.shutdown();
        }
        self.sync_connection = None;
    }

    fn route_data_packet(
        &mut self,
        incoming_sender: &mpsc::Sender<XrNetIncoming>,
        node_id: XrNetPeerId,
        packet: XrNetDataPacket,
    ) {
        if !packet.is_compatible_for(node_id) {
            return;
        }
        let peer = self.peer;
        match packet {
            XrNetDataPacket::State { frame, .. } => {
                if !accept_seq(self.last_state_seq, frame.seq) {
                    return;
                }
                self.last_state_seq = Some(frame.seq);
                let _ = incoming_sender.send(XrNetIncoming::State { peer, frame });
            }
            XrNetDataPacket::Alignment { frame, .. } => {
                if !accept_seq(self.last_alignment_seq, frame.seq) {
                    return;
                }
                self.last_alignment_seq = Some(frame.seq);
                let _ = incoming_sender.send(XrNetIncoming::Alignment { peer, frame });
            }
            XrNetDataPacket::AlignmentDescriptor { frame, .. } => {
                if !accept_seq(self.last_alignment_descriptor_seq, frame.seq) {
                    return;
                }
                self.last_alignment_descriptor_seq = Some(frame.seq);
                let _ = incoming_sender.send(XrNetIncoming::AlignmentDescriptor { peer, frame });
            }
            XrNetDataPacket::ActivityControl { control, .. } => {
                let _ = incoming_sender.send(XrNetIncoming::Activity { peer, control });
            }
            XrNetDataPacket::Leave(_) => {}
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
        let Some(payload) = XrNetSyncFrameCodec::encode(packet) else {
            return;
        };
        let frame_len = payload.len().min(u32::MAX as usize) as u32;
        self.write_buf.extend_from_slice(&frame_len.to_le_bytes());
        self.write_buf
            .extend_from_slice(&payload[..frame_len as usize]);
    }

    fn queue_hello(&mut self, node_id: XrNetPeerId) {
        self.queue_packet(&XrNetSyncPacket::hello(node_id));
    }

    fn pump(&mut self) -> io::Result<Vec<XrNetSyncPacket>> {
        self.flush()?;

        let mut read_chunk = [0u8; 16384];
        let mut total_read = 0usize;
        loop {
            match self.stream.read(&mut read_chunk) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "sync connection closed",
                    ))
                }
                Ok(len) => {
                    self.read_buf.extend_from_slice(&read_chunk[..len]);
                    total_read += len;
                    if total_read >= XR_NET_SYNC_READ_BUDGET_BYTES_PER_POLL {
                        break;
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) => return Err(err),
            }
        }
        XrNetSyncFrameCodec::drain_packets(&mut self.read_buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut total_written = 0usize;
        while !self.write_buf.is_empty() {
            match self.stream.write(&self.write_buf) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "sync connection write zero",
                    ))
                }
                Ok(written) => {
                    Self::consume_written_prefix(&mut self.write_buf, written);
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

    fn consume_written_prefix(buf: &mut Vec<u8>, len: usize) {
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

    fn shutdown(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

struct XrNetUdpWorker {
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
}

impl XrNetUdpWorker {
    fn run(self) {
        let mut peers = HashMap::<XrNetPeerId, UdpWorkerPeerState>::new();
        let mut cached_state = None;
        let mut next_discovery_at = Instant::now();
        let mut read_buf = [0u8; 65536];

        while self.thread_loop.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now >= next_discovery_at {
                self.send_discovery_hello();
                next_discovery_at = now + self.discovery_interval;
            }

            let should_break = self.drain_outgoing_messages(&mut peers, &mut cached_state);
            self.process_data_packets(&mut read_buf, &mut peers);
            self.process_discovery_packets(&mut read_buf, &mut peers, &cached_state);
            self.expire_timed_out_peers(&mut peers);

            if should_break {
                break;
            }
            thread::sleep(self.poll_interval);
        }

        self.broadcast_explicit_leave(&peers);
        self.broadcast_discovery_leave();
    }

    fn send_discovery_hello(&self) {
        let buf =
            XrNetDiscoveryPacket::hello(self.node_id, self.bound_data_port, self.bound_sync_port)
                .to_bytes();
        for target in &self.discovery_targets {
            let _ = self.discovery_socket.send_to(&buf, target);
        }
    }

    fn broadcast_discovery_leave(&self) {
        let buf = XrNetDiscoveryPacket::leave(self.node_id).to_bytes();
        for target in &self.discovery_targets {
            let _ = self.discovery_socket.send_to(&buf, target);
        }
    }

    fn broadcast_explicit_leave(&self, peers: &HashMap<XrNetPeerId, UdpWorkerPeerState>) {
        let buf = XrNetDataPacket::leave(self.node_id).to_bytes();
        for peer in peers.values() {
            let _ = self.data_socket.send_to(&buf, peer.peer.addr);
        }
    }

    fn drain_outgoing_messages(
        &self,
        peers: &mut HashMap<XrNetPeerId, UdpWorkerPeerState>,
        cached_state: &mut Option<XrNetStateFrame>,
    ) -> bool {
        loop {
            match self.outgoing_receiver.try_recv() {
                Ok(XrNetUdpOutgoing::State(frame)) => {
                    *cached_state = Some(frame.clone());
                    let buf = XrNetDataPacket::state(self.node_id, frame).to_bytes();
                    for peer in peers.values() {
                        let _ = self.data_socket.send_to(&buf, peer.peer.addr);
                    }
                }
                Ok(XrNetUdpOutgoing::Break) => return true,
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => return true,
            }
        }
    }

    fn process_discovery_packets(
        &self,
        read_buf: &mut [u8],
        peers: &mut HashMap<XrNetPeerId, UdpWorkerPeerState>,
        cached_state: &Option<XrNetStateFrame>,
    ) {
        loop {
            match self.discovery_socket.recv_from(read_buf) {
                Ok((len, source_addr)) => {
                    let Some(packet) = XrNetDiscoveryPacket::from_bytes(&read_buf[..len]) else {
                        continue;
                    };
                    match packet {
                        XrNetDiscoveryPacket::Hello(packet) => {
                            if !packet.is_compatible_for(self.node_id) {
                                continue;
                            }
                            let peer_addr = SocketAddr::new(source_addr.ip(), packet.data_port);
                            let peer_sync_addr =
                                SocketAddr::new(source_addr.ip(), packet.sync_port);
                            let (peer, is_new) =
                                self.touch_peer(peers, packet.node_id, peer_addr, peer_sync_addr);
                            if is_new {
                                self.send_cached_state_to_peer(peer, cached_state);
                            }
                        }
                        XrNetDiscoveryPacket::Leave(packet) => {
                            if !packet.is_compatible_for(self.node_id) {
                                continue;
                            }
                            self.remove_peer(peers, packet.node_id, XrNetLeaveReason::Explicit);
                        }
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return,
                Err(_) => return,
            }
        }
    }

    fn process_data_packets(
        &self,
        read_buf: &mut [u8],
        peers: &mut HashMap<XrNetPeerId, UdpWorkerPeerState>,
    ) {
        loop {
            match self.data_socket.recv_from(read_buf) {
                Ok((len, source_addr)) => {
                    let Some(packet) = XrNetDataPacket::from_bytes(&read_buf[..len]) else {
                        continue;
                    };
                    if !packet.is_compatible_for(self.node_id) {
                        continue;
                    }
                    match packet {
                        XrNetDataPacket::State {
                            node_id: remote_id,
                            frame,
                            ..
                        } => {
                            let sync_addr = peers
                                .get(&remote_id)
                                .map(|peer_state| peer_state.sync_addr)
                                .unwrap_or_else(|| {
                                    SocketAddr::new(source_addr.ip(), XR_NET_DEFAULT_SYNC_PORT)
                                });
                            let (peer, _) =
                                self.touch_peer(peers, remote_id, source_addr, sync_addr);
                            let Some(peer_state) = peers.get_mut(&remote_id) else {
                                continue;
                            };
                            if !accept_seq(peer_state.last_state_seq, frame.seq) {
                                continue;
                            }
                            peer_state.last_state_seq = Some(frame.seq);
                            let _ = self
                                .incoming_sender
                                .send(XrNetIncoming::State { peer, frame });
                        }
                        XrNetDataPacket::Alignment {
                            node_id: remote_id,
                            frame,
                            ..
                        } => {
                            let sync_addr = peers
                                .get(&remote_id)
                                .map(|peer_state| peer_state.sync_addr)
                                .unwrap_or_else(|| {
                                    SocketAddr::new(source_addr.ip(), XR_NET_DEFAULT_SYNC_PORT)
                                });
                            let (peer, _) =
                                self.touch_peer(peers, remote_id, source_addr, sync_addr);
                            let Some(peer_state) = peers.get_mut(&remote_id) else {
                                continue;
                            };
                            if !accept_seq(peer_state.last_alignment_seq, frame.seq) {
                                continue;
                            }
                            peer_state.last_alignment_seq = Some(frame.seq);
                            let _ = self
                                .incoming_sender
                                .send(XrNetIncoming::Alignment { peer, frame });
                        }
                        XrNetDataPacket::AlignmentDescriptor {
                            node_id: remote_id,
                            frame,
                            ..
                        } => {
                            let sync_addr = peers
                                .get(&remote_id)
                                .map(|peer_state| peer_state.sync_addr)
                                .unwrap_or_else(|| {
                                    SocketAddr::new(source_addr.ip(), XR_NET_DEFAULT_SYNC_PORT)
                                });
                            let (peer, _) =
                                self.touch_peer(peers, remote_id, source_addr, sync_addr);
                            let Some(peer_state) = peers.get_mut(&remote_id) else {
                                continue;
                            };
                            if !accept_seq(peer_state.last_alignment_descriptor_seq, frame.seq) {
                                continue;
                            }
                            peer_state.last_alignment_descriptor_seq = Some(frame.seq);
                            let _ = self
                                .incoming_sender
                                .send(XrNetIncoming::AlignmentDescriptor { peer, frame });
                        }
                        XrNetDataPacket::Leave(packet) => {
                            self.remove_peer(peers, packet.node_id, XrNetLeaveReason::Explicit);
                        }
                        XrNetDataPacket::ActivityControl { .. } => {}
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return,
                Err(_) => return,
            }
        }
    }

    fn touch_peer(
        &self,
        peers: &mut HashMap<XrNetPeerId, UdpWorkerPeerState>,
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
                let _ = self.sync_outgoing_sender.send(XrNetSyncOutgoing::PeerUp {
                    peer: peer_state.peer,
                    sync_addr,
                });
            }
            (peer_state.peer, false)
        } else {
            let peer = XrNetPeer { id: peer_id, addr };
            peers.insert(peer_id, UdpWorkerPeerState::new(peer, sync_addr, now));
            let _ = self.incoming_sender.send(XrNetIncoming::Join { peer });
            let _ = self
                .sync_outgoing_sender
                .send(XrNetSyncOutgoing::PeerUp { peer, sync_addr });
            (peer, true)
        }
    }

    fn send_cached_state_to_peer(&self, peer: XrNetPeer, cached_state: &Option<XrNetStateFrame>) {
        if let Some(frame) = cached_state {
            let _ = self.data_socket.send_to(
                &XrNetDataPacket::state(self.node_id, frame.clone()).to_bytes(),
                peer.addr,
            );
        }
    }

    fn remove_peer(
        &self,
        peers: &mut HashMap<XrNetPeerId, UdpWorkerPeerState>,
        peer_id: XrNetPeerId,
        reason: XrNetLeaveReason,
    ) {
        let Some(peer_state) = peers.remove(&peer_id) else {
            return;
        };
        let _ = self
            .sync_outgoing_sender
            .send(XrNetSyncOutgoing::PeerRemoved { peer_id });
        let _ = self.incoming_sender.send(XrNetIncoming::Leave {
            peer: peer_state.peer,
            reason,
        });
    }

    fn expire_timed_out_peers(&self, peers: &mut HashMap<XrNetPeerId, UdpWorkerPeerState>) {
        let now = Instant::now();
        let expired: Vec<_> = peers
            .iter()
            .filter_map(|(peer_id, peer_state)| {
                (now.duration_since(peer_state.last_seen) > self.peer_timeout).then_some(*peer_id)
            })
            .collect();
        for peer_id in expired {
            self.remove_peer(peers, peer_id, XrNetLeaveReason::Timeout);
        }
    }
}

struct XrNetSyncWorker {
    node_id: XrNetPeerId,
    poll_interval: Duration,
    sync_listener: TcpListener,
    incoming_sender: mpsc::Sender<XrNetIncoming>,
    outgoing_receiver: mpsc::Receiver<XrNetSyncOutgoing>,
    thread_loop: Arc<AtomicBool>,
}

impl XrNetSyncWorker {
    fn run(self) {
        let mut peers = HashMap::<XrNetPeerId, SyncWorkerPeerState>::new();
        let mut pending_sync_connections = Vec::<XrNetSyncConnection>::new();
        let mut cached_alignment = None;
        let mut cached_alignment_descriptor = None;
        let mut cached_activity = None;

        while self.thread_loop.load(Ordering::Relaxed) {
            let should_break = self.drain_outgoing_messages(
                &mut peers,
                &mut cached_alignment,
                &mut cached_alignment_descriptor,
                &mut cached_activity,
            );
            self.accept_connections(&mut pending_sync_connections);
            self.ensure_outbound_connections(&mut peers);
            self.process_pending_connections(
                &mut pending_sync_connections,
                &mut peers,
                &cached_alignment,
                &cached_alignment_descriptor,
                &cached_activity,
            );
            self.process_connections(
                &mut peers,
                &cached_alignment,
                &cached_alignment_descriptor,
                &cached_activity,
            );

            if should_break {
                break;
            }
            thread::sleep(self.poll_interval);
        }

        for peer_state in peers.values_mut() {
            peer_state.close_connection();
        }
        for pending in &mut pending_sync_connections {
            pending.shutdown();
        }
    }

    fn drain_outgoing_messages(
        &self,
        peers: &mut HashMap<XrNetPeerId, SyncWorkerPeerState>,
        cached_alignment: &mut Option<XrNetAlignmentFrame>,
        cached_alignment_descriptor: &mut Option<XrNetAlignmentDescriptorFrame>,
        cached_activity: &mut Option<XrNetActivityControl>,
    ) -> bool {
        loop {
            match self.outgoing_receiver.try_recv() {
                Ok(XrNetSyncOutgoing::PeerUp { peer, sync_addr }) => {
                    self.register_peer(peers, peer, sync_addr);
                }
                Ok(XrNetSyncOutgoing::PeerRemoved { peer_id }) => {
                    self.remove_peer(peers, peer_id);
                }
                Ok(XrNetSyncOutgoing::Alignment(frame)) => {
                    *cached_alignment = Some(frame);
                    let packet = XrNetDataPacket::alignment(self.node_id, frame);
                    for peer_state in peers.values_mut() {
                        if let Some(connection) = peer_state.sync_connection.as_mut() {
                            connection.queue_packet(&XrNetSyncPacket::data(packet.clone()));
                        }
                    }
                }
                Ok(XrNetSyncOutgoing::AlignmentDescriptor(frame)) => {
                    *cached_alignment_descriptor = Some(frame.clone());
                    let packet = XrNetDataPacket::alignment_descriptor(self.node_id, frame);
                    for peer_state in peers.values_mut() {
                        if let Some(connection) = peer_state.sync_connection.as_mut() {
                            connection.queue_packet(&XrNetSyncPacket::data(packet.clone()));
                        }
                    }
                }
                Ok(XrNetSyncOutgoing::Activity(control)) => {
                    *cached_activity = Some(control.clone());
                    let packet = XrNetDataPacket::activity_control(self.node_id, control);
                    for peer_state in peers.values_mut() {
                        if let Some(connection) = peer_state.sync_connection.as_mut() {
                            connection.queue_packet(&XrNetSyncPacket::data(packet.clone()));
                        }
                    }
                }
                Ok(XrNetSyncOutgoing::Break) => return true,
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => return true,
            }
        }
    }

    fn register_peer(
        &self,
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
                peer_state.close_connection();
                peer_state.next_sync_connect_attempt_at = now;
            }
        } else {
            peers.insert(peer.id, SyncWorkerPeerState::new(peer, sync_addr, now));
        }
    }

    fn remove_peer(
        &self,
        peers: &mut HashMap<XrNetPeerId, SyncWorkerPeerState>,
        peer_id: XrNetPeerId,
    ) {
        let Some(mut peer_state) = peers.remove(&peer_id) else {
            return;
        };
        peer_state.close_connection();
    }

    fn accept_connections(&self, pending_sync_connections: &mut Vec<XrNetSyncConnection>) {
        loop {
            match self.sync_listener.accept() {
                Ok((stream, _)) => {
                    let Ok(mut connection) = XrNetSyncConnection::new(stream, None) else {
                        continue;
                    };
                    connection.queue_hello(self.node_id);
                    pending_sync_connections.push(connection);
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return,
                Err(_) => return,
            }
        }
    }

    fn ensure_outbound_connections(&self, peers: &mut HashMap<XrNetPeerId, SyncWorkerPeerState>) {
        let now = Instant::now();
        let peer_ids = peers.keys().copied().collect::<Vec<_>>();
        for peer_id in peer_ids {
            let Some(peer_state) = peers.get_mut(&peer_id) else {
                continue;
            };
            if peer_state.sync_connection.is_some()
                || !Self::should_initiate_connection(self.node_id, peer_id)
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
            connection.queue_hello(self.node_id);
            peer_state.sync_connection = Some(connection);
        }
    }

    fn process_pending_connections(
        &self,
        pending_sync_connections: &mut Vec<XrNetSyncConnection>,
        peers: &mut HashMap<XrNetPeerId, SyncWorkerPeerState>,
        cached_alignment: &Option<XrNetAlignmentFrame>,
        cached_alignment_descriptor: &Option<XrNetAlignmentDescriptorFrame>,
        cached_activity: &Option<XrNetActivityControl>,
    ) {
        let pending_count = pending_sync_connections.len();
        for _ in 0..pending_count {
            let mut connection = pending_sync_connections.swap_remove(0);
            let Ok(packets) = connection.pump() else {
                connection.shutdown();
                continue;
            };
            let mut queued_data = Vec::<XrNetDataPacket>::new();
            let mut invalid = false;

            for packet in packets {
                match packet {
                    XrNetSyncPacket::Hello(hello) => {
                        if !hello.is_compatible_for(self.node_id) {
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

            peer_state.close_connection();
            connection.handshake_received = true;
            self.send_cached_frames_to_connection(
                &mut connection,
                cached_alignment,
                cached_alignment_descriptor,
                cached_activity,
            );
            peer_state.sync_connection = Some(connection);

            for data in queued_data {
                peer_state.route_data_packet(&self.incoming_sender, self.node_id, data);
            }
        }
    }

    fn process_connections(
        &self,
        peers: &mut HashMap<XrNetPeerId, SyncWorkerPeerState>,
        cached_alignment: &Option<XrNetAlignmentFrame>,
        cached_alignment_descriptor: &Option<XrNetAlignmentDescriptorFrame>,
        cached_activity: &Option<XrNetActivityControl>,
    ) {
        let peer_ids = peers.keys().copied().collect::<Vec<_>>();
        for peer_id in peer_ids {
            let Some(mut connection) = peers
                .get_mut(&peer_id)
                .and_then(|peer_state| peer_state.sync_connection.take())
            else {
                continue;
            };

            let Ok(packets) = connection.pump() else {
                connection.shutdown();
                continue;
            };

            let mut invalid = false;
            let mut became_ready = false;
            let mut queued_data = Vec::<XrNetDataPacket>::new();
            for packet in packets {
                match packet {
                    XrNetSyncPacket::Hello(hello) => {
                        if !hello.matches_peer(peer_id) {
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
                self.send_cached_frames_to_connection(
                    &mut connection,
                    cached_alignment,
                    cached_alignment_descriptor,
                    cached_activity,
                );
            }
            for data in queued_data {
                peer_state.route_data_packet(&self.incoming_sender, self.node_id, data);
            }
            peer_state.sync_connection = Some(connection);
        }
    }

    fn send_cached_frames_to_connection(
        &self,
        connection: &mut XrNetSyncConnection,
        cached_alignment: &Option<XrNetAlignmentFrame>,
        cached_alignment_descriptor: &Option<XrNetAlignmentDescriptorFrame>,
        cached_activity: &Option<XrNetActivityControl>,
    ) {
        if let Some(frame) = cached_alignment {
            connection.queue_packet(&XrNetSyncPacket::data(XrNetDataPacket::alignment(
                self.node_id,
                *frame,
            )));
        }
        if let Some(frame) = cached_alignment_descriptor {
            connection.queue_packet(&XrNetSyncPacket::data(
                XrNetDataPacket::alignment_descriptor(self.node_id, frame.clone()),
            ));
        }
        if let Some(control) = cached_activity {
            connection.queue_packet(&XrNetSyncPacket::data(XrNetDataPacket::activity_control(
                self.node_id,
                control.clone(),
            )));
        }
    }

    fn should_initiate_connection(local_node_id: XrNetPeerId, peer_id: XrNetPeerId) -> bool {
        local_node_id.0 < peer_id.0
    }
}

pub(super) fn spawn_udp_worker_thread(
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
        XrNetUdpWorker {
            node_id,
            discovery_targets,
            discovery_interval,
            peer_timeout,
            poll_interval,
            bound_data_port,
            bound_sync_port,
            discovery_socket,
            data_socket,
            incoming_sender,
            outgoing_receiver,
            sync_outgoing_sender,
            thread_loop,
        }
        .run();
    })
}

pub(super) fn spawn_sync_worker_thread(
    node_id: XrNetPeerId,
    poll_interval: Duration,
    sync_listener: TcpListener,
    incoming_sender: mpsc::Sender<XrNetIncoming>,
    outgoing_receiver: mpsc::Receiver<XrNetSyncOutgoing>,
    thread_loop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        XrNetSyncWorker {
            node_id,
            poll_interval,
            sync_listener,
            incoming_sender,
            outgoing_receiver,
            thread_loop,
        }
        .run();
    })
}
