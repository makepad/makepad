//! Owned iteration children share GPU frames over the existing Studio protocol.
//! A single worker owns all sockets; neither the UI nor the build reactor blocks.

use makepad_widgets::makepad_micro_serde::{DeBin, SerBin};
use makepad_widgets::makepad_platform::{
    makepad_network::web_socket_parser::WebSocketParser,
    studio::{AppToStudio, AppToStudioVec, StudioToApp, StudioToAppVec},
    thread::{SignalToUI, TaskHandle, ThreadOptions, ThreadSpawner},
};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
    time::{Duration, Instant},
};

pub type ClientId = u64;
const CHANNEL_CAPACITY: usize = 128;
const MAX_CLIENTS: usize = 32;
const MAX_SOCKETS: usize = 36;
const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_HEADER: usize = 8192;
const MAX_PENDING_EVENTS: usize = 64;

#[derive(Debug)]
pub enum HostCommand {
    /// Await Registered before launching the retained binary with this ID.
    Register { client: ClientId, run_id: String },
    Unregister { client: ClientId },
    Send { client: ClientId, data: Arc<Vec<u8>> },
}

impl HostCommand {
    pub fn messages(client: ClientId, messages: Vec<StudioToApp>) -> Self {
        Self::Send { client, data: Arc::new(StudioToAppVec(messages).serialize_bin()) }
    }
}

#[derive(Debug)]
pub enum HostEvent {
    Listening { port: u16 },
    Registered { client: ClientId, run_id: String },
    Connected { client: ClientId },
    Disconnected { client: ClientId, reason: String },
    FromApp { client: ClientId, messages: Arc<Vec<AppToStudio>> },
    Error(String),
}

pub struct IterationHost {
    sender: SyncSender<HostCommand>,
    receiver: Receiver<HostEvent>,
    stop: Arc<AtomicBool>,
    worker: TaskHandle<()>,
}

impl IterationHost {
    /// Binding occurs on the worker. Listening supplies its actual owned port.
    pub fn start(spawner: &ThreadSpawner) -> Result<Self, String> {
        let (sender, commands) = mpsc::sync_channel(CHANNEL_CAPACITY);
        let (events, receiver) = mpsc::sync_channel(CHANNEL_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = spawner.spawn_worker(
            ThreadOptions { name: Some("studio-iteration-host".into()), ..Default::default() },
            move || run_host(commands, events, worker_stop),
        ).map_err(|error| error.to_string())?;
        Ok(Self { sender, receiver, stop, worker })
    }

    /// A full queue returns ownership; the UI retains and retries this command.
    pub fn try_send(&self, command: HostCommand) -> Result<(), TrySendError<HostCommand>> {
        self.sender.try_send(command)
    }

    pub fn try_recv(&self) -> Result<HostEvent, TryRecvError> { self.receiver.try_recv() }
    pub fn command_sender(&self) -> SyncSender<HostCommand> { self.sender.clone() }
    pub fn request_stop(&self) { self.stop.store(true, Ordering::Release); }
    pub fn is_finished(&self) -> bool { self.worker.is_finished() }
}

impl Drop for IterationHost {
    fn drop(&mut self) { self.stop.store(true, Ordering::Release); }
}

struct Peer {
    stream: TcpStream,
    client: Option<ClientId>,
    input: Vec<u8>,
    output: VecDeque<Vec<u8>>,
    output_offset: usize,
    output_bytes: usize,
    fragment: Vec<u8>,
    fragment_opcode: Option<u8>,
    opened: Instant,
}

impl Peer {
    fn new(stream: TcpStream) -> Result<Self, String> {
        stream.set_nonblocking(true).map_err(|error| error.to_string())?;
        stream.set_nodelay(true).map_err(|error| error.to_string())?;
        Ok(Self {
            stream, client: None, input: Vec::new(), output: VecDeque::new(),
            output_offset: 0, output_bytes: 0, fragment: Vec::new(),
            fragment_opcode: None, opened: Instant::now(),
        })
    }

    fn queue(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        if self.output_bytes.saturating_add(bytes.len()) > MAX_MESSAGE * 2 || self.output.len() >= 256 {
            return Err("Hosted child stopped consuming its bounded protocol queue".into());
        }
        self.output_bytes += bytes.len();
        self.output.push_back(bytes);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        // Fairness: a large producer cannot monopolize the reactor.
        for _ in 0..8 {
            let Some(bytes) = self.output.front() else { break; };
            match self.stream.write(&bytes[self.output_offset..]) {
                Ok(0) => return Err("Hosted child socket closed".into()),
                Ok(count) => {
                    self.output_offset += count;
                    self.output_bytes -= count;
                    if self.output_offset == bytes.len() {
                        self.output.pop_front();
                        self.output_offset = 0;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(())
    }

    fn read(&mut self) -> Result<(), String> {
        let mut bytes = [0u8; 16 * 1024];
        match self.stream.read(&mut bytes) {
            Ok(0) => return Err("Hosted child disconnected".into()),
            Ok(count) => {
                if self.input.len().saturating_add(count) > MAX_MESSAGE + 16 * 1024 {
                    return Err("Hosted child exceeded the protocol input limit".into());
                }
                self.input.extend_from_slice(&bytes[..count]);
            }
            Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {},
            Err(error) => return Err(error.to_string()),
        }
        Ok(())
    }

    fn handshake(&mut self, registered: &BTreeMap<ClientId, String>) -> Result<Option<ClientId>, String> {
        if self.opened.elapsed() > Duration::from_secs(5) {
            return Err("Hosted child handshake timed out".into());
        }
        let Some(end) = self.input.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
            if self.input.len() > MAX_HEADER { return Err("Hosted child HTTP headers are too large".into()); }
            return Ok(None);
        };
        if end > MAX_HEADER { return Err("Hosted child HTTP headers are too large".into()); }
        let header = std::str::from_utf8(&self.input[..end]).map_err(|_| "Invalid hosted child HTTP headers")?;
        let mut lines = header.split("\r\n");
        let mut request = lines.next().unwrap_or_default().split_whitespace();
        if request.next() != Some("GET") { return Err("Only the owned app websocket is available".into()); }
        let client = parse_client_path(request.next().unwrap_or_default()).ok_or("Missing hosted client ID")?;
        if !registered.contains_key(&client) { return Err("Unregistered hosted client ID".into()); }
        let mut key = None;
        let mut upgrade = false;
        let mut connection = false;
        for line in lines {
            let Some((name, value)) = line.split_once(':') else { continue; };
            if name.eq_ignore_ascii_case("sec-websocket-key") { key = Some(value.trim()); }
            if name.eq_ignore_ascii_case("upgrade") { upgrade = value.trim().eq_ignore_ascii_case("websocket"); }
            if name.eq_ignore_ascii_case("connection") { connection = value.split(',').any(|word| word.trim().eq_ignore_ascii_case("upgrade")); }
        }
        let key = key.filter(|key| (16..=64).contains(&key.len())).ok_or("Missing websocket key")?;
        if !upgrade || !connection { return Err("Expected a websocket upgrade".into()); }
        let response = WebSocketParser::create_upgrade_response(key).into_bytes();
        self.input.drain(..end + 4);
        self.queue(response)?;
        self.client = Some(client);
        Ok(Some(client))
    }

    fn receive_message(&mut self) -> Result<Option<Vec<u8>>, String> {
        if self.input.len() < 2 { return Ok(None); }
        let first = self.input[0];
        if first & 0x70 != 0 { return Err("Unsupported websocket extension".into()); }
        let opcode = first & 15;
        let final_frame = first & 128 != 0;
        let masked = self.input[1] & 128 != 0;
        let mut header = 2usize;
        let length = match self.input[1] & 127 {
            126 => {
                if self.input.len() < 4 { return Ok(None); }
                header = 4;
                u16::from_be_bytes([self.input[2], self.input[3]]) as u64
            }
            127 => {
                if self.input.len() < 10 { return Ok(None); }
                header = 10;
                u64::from_be_bytes(self.input[2..10].try_into().unwrap())
            }
            value => u64::from(value),
        };
        if length > MAX_MESSAGE as u64 { return Err("Hosted websocket frame exceeds 1MiB".into()); }
        if opcode >= 8 && (!final_frame || length > 125) { return Err("Invalid websocket control frame".into()); }
        let mask_offset = header;
        if masked { header += 4; }
        let end = header + length as usize;
        if self.input.len() < end { return Ok(None); }
        let mut payload = self.input[header..end].to_vec();
        if masked {
            for (index, byte) in payload.iter_mut().enumerate() { *byte ^= self.input[mask_offset + index % 4]; }
        }
        // Makepad's existing loopback websocket backend also sends unmasked
        // frames; accept those only on this loopback-only registered service.
        self.input.drain(..end);
        match opcode {
            8 => Err("Hosted child closed its websocket".into()),
            9 => { self.queue(websocket_frame(10, &payload))?; Ok(Some(Vec::new())) },
            10 => Ok(Some(Vec::new())),
            0 | 1 | 2 => {
                if opcode != 0 {
                    if self.fragment_opcode.is_some() { return Err("Interleaved websocket fragments".into()); }
                    self.fragment_opcode = Some(opcode);
                } else if self.fragment_opcode.is_none() { return Err("Unexpected websocket continuation".into()); }
                if self.fragment.len().saturating_add(payload.len()) > MAX_MESSAGE { return Err("Hosted message exceeds 1MiB".into()); }
                self.fragment.extend_from_slice(&payload);
                if !final_frame { return Ok(Some(Vec::new())); }
                let kind = self.fragment_opcode.take();
                let message = std::mem::take(&mut self.fragment);
                // Studio's child protocol is binary. Text has no host powers.
                Ok(Some(if kind == Some(2) { message } else { Vec::new() }))
            }
            _ => Err("Unsupported websocket opcode".into()),
        }
    }
}

fn run_host(commands: Receiver<HostCommand>, events: SyncSender<HostEvent>, stop: Arc<AtomicBool>) {
    let listener = match TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)) {
        Ok(listener) => listener,
        Err(error) => { let _ = events.try_send(HostEvent::Error(format!("Cannot start iteration host: {error}"))); SignalToUI::set_ui_signal(); return; }
    };
    let address = match listener.local_addr().and_then(|address| listener.set_nonblocking(true).map(|_| address)) {
        Ok(address) => address,
        Err(error) => { let _ = events.try_send(HostEvent::Error(error.to_string())); SignalToUI::set_ui_signal(); return; }
    };
    let mut pending = VecDeque::from([HostEvent::Listening { port: address.port() }]);
    let mut registrations = BTreeMap::<ClientId, String>::new();
    let mut peers = BTreeMap::<u64, Peer>::new();
    let mut next_socket = 0u64;
    while !stop.load(Ordering::Acquire) {
        while let Some(event) = pending.pop_front() {
            match events.try_send(event) {
                Ok(()) => SignalToUI::set_ui_signal(),
                Err(TrySendError::Full(event)) => { pending.push_front(event); break; },
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
        // Backpressure reaches the sockets. Retain a bounded event tail and
        // stop accepting/reading until the UI has caught up.
        if pending.len() < MAX_PENDING_EVENTS / 2 {
            for _ in 0..16 {
                let command = match commands.try_recv() {
                    Ok(command) => command,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                };
                match command {
                    HostCommand::Register { client, run_id } => {
                        if client == 0 || run_id.is_empty() || run_id.len() > 96 || !run_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) {
                            pending.push_back(HostEvent::Error("Invalid hosted run identity".into()));
                        } else if registrations.get(&client).is_some_and(|existing| existing != &run_id) {
                            pending.push_back(HostEvent::Error("Hosted client ID already belongs to another run".into()));
                        } else if registrations.iter().any(|(existing_client, existing_run)| *existing_client != client && existing_run == &run_id) {
                            pending.push_back(HostEvent::Error("Hosted run already has a registered client ID".into()));
                        } else if registrations.len() >= MAX_CLIENTS && !registrations.contains_key(&client) {
                            pending.push_back(HostEvent::Error("Hosted run capacity reached; unregister closed runs".into()));
                        } else {
                            registrations.insert(client, run_id.clone());
                            pending.push_back(HostEvent::Registered { client, run_id });
                        }
                    }
                    HostCommand::Unregister { client } => {
                        registrations.remove(&client);
                        peers.retain(|_, peer| peer.client != Some(client));
                    }
                    HostCommand::Send { client, data } => {
                        if data.len() > MAX_MESSAGE {
                            pending.push_back(HostEvent::Error("Outbound hosted message exceeds 1MiB".into()));
                            continue;
                        }
                        let socket = peers.iter().find_map(|(socket, peer)| (peer.client == Some(client)).then_some(*socket));
                        if let Some(socket) = socket {
                            if let Err(reason) = peers.get_mut(&socket).unwrap().queue(websocket_frame(2, &data)) {
                                peers.remove(&socket);
                                pending.push_back(HostEvent::Disconnected { client, reason });
                            }
                        }
                    }
                }
            }
            for _ in 0..4 {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if peers.len() >= MAX_SOCKETS { drop(stream); continue; }
                        if let Ok(peer) = Peer::new(stream) { next_socket += 1; peers.insert(next_socket, peer); }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }
        }
        let sockets = peers.keys().copied().collect::<Vec<_>>();
        for socket in sockets {
            let connected = peers.values().filter_map(|peer| peer.client).collect::<Vec<_>>();
            let Some(peer) = peers.get_mut(&socket) else { continue; };
            let result = (|| {
                peer.flush()?;
                if pending.len() >= MAX_PENDING_EVENTS - 8 { return Ok(()); }
                peer.read()?;
                if peer.client.is_none() {
                    if let Some(client) = peer.handshake(&registrations)? {
                        if connected.contains(&client) {
                            peer.client = None;
                            return Err("Hosted run already has a connected process".into());
                        }
                        pending.push_back(HostEvent::Connected { client });
                    } else { return Ok(()); }
                }
                let client = peer.client.unwrap();
                for _ in 0..4 {
                    let Some(data) = peer.receive_message()? else { break; };
                    if data.is_empty() { continue; }
                    let messages = decode_messages(&data)?;
                    pending.push_back(HostEvent::FromApp { client, messages: Arc::new(messages) });
                }
                Ok::<(), String>(())
            })();
            if let Err(reason) = result {
                if let Some(client) = peer.client { pending.push_back(HostEvent::Disconnected { client, reason }); }
                peers.remove(&socket);
            }
        }
        // One long-lived reactor, never one thread per connection or command.
        std::thread::sleep(Duration::from_millis(4));
    }
}

fn decode_messages(bytes: &[u8]) -> Result<Vec<AppToStudio>, String> {
    let mut offset = 0;
    if let Ok(messages) = AppToStudioVec::de_bin(&mut offset, bytes) {
        if offset == bytes.len() && messages.0.len() <= 256 { return Ok(messages.0); }
    }
    offset = 0;
    if let Ok(message) = AppToStudio::de_bin(&mut offset, bytes) {
        if offset == bytes.len() { return Ok(vec![message]); }
    }
    Err("Invalid or oversized hosted app protocol message".into())
}

fn websocket_frame(opcode: u8, bytes: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(bytes.len() + 10);
    frame.push(128 | opcode);
    if bytes.len() < 126 { frame.push(bytes.len() as u8); }
    else if bytes.len() <= u16::MAX as usize { frame.push(126); frame.extend_from_slice(&(bytes.len() as u16).to_be_bytes()); }
    else { frame.push(127); frame.extend_from_slice(&(bytes.len() as u64).to_be_bytes()); }
    frame.extend_from_slice(bytes);
    frame
}

fn parse_client_path(path: &str) -> Option<ClientId> {
    if let Some(value) = path.strip_prefix("/app/") { return value.parse().ok(); }
    let (route, query) = path.split_once('?')?;
    if route != "/app" { return None; }
    query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == "build").then(|| value.parse().ok()).flatten()
    })
}
