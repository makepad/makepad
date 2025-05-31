// ok so everyone sends interaction events to the system
// and the system has physics 'state' as output. 
// so how do we do this
use{
    std::io,
    std::net::{UdpSocket,IpAddr,Ipv4Addr},
    std::sync::mpsc,
    std::time::{Duration, Instant},
    std::net::SocketAddr,
    std::thread,
    std::sync::Arc,
    std::sync::atomic::{AtomicBool,Ordering},
    makepad_platform::*,
    makepad_platform::{
        makepad_micro_serde::*,
    },
};
/*
pub enum XrNetEvent{
    CreateObject{
        object: XrNetObject,
    },
    DestroyObject{
        id: LiveId,
    },
    MoveObject{
        id: LiveId,
        pose: Pose
    }
}

pub struct XrNetObject{
    pub id: LiveId,
    pub shape: XrNetShape,
    pub pose: Pose
}

pub enum XrNetShape{
    Cube{size: f32}
}*/

#[derive(Clone, Copy, Debug)]
pub struct XrNetPeer{
    addr: SocketAddr,
}
impl XrNetPeer{
    pub fn to_live_id(&self)->LiveId{
        LiveId::from_str(&format!("{:?}",self.addr))
    }
}

#[derive(Clone, Debug)]
pub enum XrNetIncoming{
    Join{peer:XrNetPeer, state:XrState},
    Leave{peer:XrNetPeer},
    Update{peer:XrNetPeer, state:XrState},
}

#[derive(Debug)]
pub struct XrNetNode{
    pub outgoing_sender: mpsc::Sender<XrNetOutgoing>,
    pub incoming_receiver: mpsc::Receiver<XrNetIncoming>,
    pub discovery_write_thread: Option<thread::JoinHandle<()>>,
    pub discovery_read_thread:  Option<thread::JoinHandle<()>>,
    pub state_read_thread:  Option<thread::JoinHandle<()>>,
    pub state_write_thread:  Option<thread::JoinHandle<()>>,
    pub thread_loop: Arc<AtomicBool>
}

impl Drop for XrNetNode{
    fn drop(&mut self){
        self.thread_loop.store(false, Ordering::Relaxed);
        // Attempt to send Break, but don't panic if channel is closed
        let _ = self.outgoing_sender.send(XrNetOutgoing::Break);
        
        // Join threads, ignoring errors from join (e.g. if a thread panicked)
        if let Some(handle) = self.discovery_write_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.discovery_read_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.state_read_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.state_write_thread.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Clone, Debug)]
pub enum XrNetOutgoing{
    Discovered(XrNetPeer),
    Leave(XrNetPeer),
    State(XrState),
    Break,
}        

const PEER_LEAVE_TIMEOUT: f64 = 2.0;
const DISCOVERY_PORT: u16 = 41546;
const STATE_PORT: u16 = 41547;

impl XrNetNode{
    pub fn send_state(&mut self, state:XrState){
        self.outgoing_sender.send(XrNetOutgoing::State(state)).ok();
    }
    
    pub fn new(_cx:&mut Cx)->Self{
        let discovery_bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), DISCOVERY_PORT);
        let discovery_send_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)), DISCOVERY_PORT);
        let state_bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), STATE_PORT);
        
        let my_client_uid = LiveId::from_str(&format!("{:?}", std::time::SystemTime::now())).0;
        
        let thread_loop = Arc::new(AtomicBool::new(true));
        let (outgoing_sender, outgoing_receiver) = mpsc::channel();
        let (incoming_sender, incoming_receiver_val) = mpsc::channel(); // Renamed to avoid conflict

        // Discovery Write Thread
        let tl_dw = thread_loop.clone();
        let mc_uid_dw = my_client_uid;
        let ds_addr_dw = discovery_send_addr;
        let discovery_write_thread = std::thread::spawn(move || {
            let mut socket: Option<UdpSocket> = None;
            loop {
                if !tl_dw.load(Ordering::Relaxed) { break; }

                if socket.is_none() {
                    match UdpSocket::bind("0.0.0.0:0") {
                        Ok(s) => {
                            if let Err(e) = s.set_broadcast(true) {
                                eprintln!("DW: Failed to set broadcast: {:?}. Retrying socket creation.", e);
                                socket = None; 
                                thread::sleep(Duration::from_secs(1));
                                continue;
                            }
                            socket = Some(s);
                        }
                        Err(e) => {
                            eprintln!("DW: Failed to bind discovery write socket: {:?}. Retrying in 1s.", e);
                            thread::sleep(Duration::from_secs(1));
                            continue;
                        }
                    }
                }

                if let Some(s) = &socket {
                    match s.send_to(&mc_uid_dw.to_be_bytes(), ds_addr_dw) {
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("DW: Send error: {:?}. Re-creating socket.", e);
                            socket = None; 
                            thread::sleep(Duration::from_millis(100)); 
                            continue; 
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs_f64(0.1));
            }
        });

        // Discovery Read Thread
        let tl_dr = thread_loop.clone();
        let mc_uid_dr = my_client_uid;
        let db_addr_dr = discovery_bind_addr;
        let os_dr = outgoing_sender.clone();
        let discovery_read_thread = std::thread::spawn(move || {
            let mut socket: Option<UdpSocket> = None;
            let mut read_buf = [0u8; 4096];
            loop {
                if !tl_dr.load(Ordering::Relaxed) { break; }

                if socket.is_none() {
                    match UdpSocket::bind(db_addr_dr) {
                        Ok(s) => {
                            if let Err(e) = s.set_read_timeout(Some(Duration::from_secs_f64(0.25))) {
                                eprintln!("DR: Failed to set read timeout: {:?}. Retrying socket creation.", e);
                                socket = None; thread::sleep(Duration::from_secs(1)); continue;
                            }
                             if let Err(e) = s.set_broadcast(true) { 
                                eprintln!("DR: Failed to set broadcast: {:?}. Retrying socket creation.", e);
                                socket = None; thread::sleep(Duration::from_secs(1)); continue;
                            }
                            socket = Some(s);
                        }
                        Err(e) => {
                            eprintln!("DR: Failed to bind discovery read socket: {:?}. Retrying in 1s.", e);
                            thread::sleep(Duration::from_secs(1));
                            continue;
                        }
                    }
                }

                if let Some(s) = &socket {
                    match s.recv_from(&mut read_buf) {
                        Ok((len, mut peer_addr)) => {
                            if len == std::mem::size_of::<u64>() {
                                let received_uid_bytes: [u8; 8] = read_buf[0..8].try_into().expect("Slice size known to be 8");
                                let received_uid = u64::from_be_bytes(received_uid_bytes);
                                if received_uid != mc_uid_dr {
                                    peer_addr.set_port(STATE_PORT);
                                    let peer = XrNetPeer { addr: peer_addr };
                                    os_dr.send(XrNetOutgoing::Discovered(peer)).ok();
                                }
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => { /* Expected, continue */ }
                        Err(e) => {
                            eprintln!("DR: Recv error: {:?}. Re-creating socket.", e);
                            socket = None; 
                        }
                    }
                } else {
                     thread::sleep(Duration::from_millis(100)); // Socket is None, brief pause before retry
                }
            }
        });
        
        // State Read Thread
        let tl_sr = thread_loop.clone();
        let sb_addr_sr = state_bind_addr;
        let is_sr = incoming_sender; // Move original incoming_sender here
        let os_sr_leave = outgoing_sender.clone(); 
        let state_read_thread = std::thread::spawn(move || {
            let mut socket: Option<UdpSocket> = None;
            let mut read_buf = [0u8; 4096];
            struct ActivePeer { addr: SocketAddr, last_seen: Instant }
            let mut active_peers: Vec<ActivePeer> = Vec::new();

            loop {
                if !tl_sr.load(Ordering::Relaxed) { break; }

                if socket.is_none() {
                    match UdpSocket::bind(sb_addr_sr) {
                        Ok(s) => {
                             if let Err(e) = s.set_read_timeout(Some(Duration::from_secs_f64(0.25))) {
                                eprintln!("SR: Failed to set read timeout: {:?}. Retrying socket creation.", e);
                                socket = None; thread::sleep(Duration::from_secs(1)); continue;
                            }
                            socket = Some(s);
                        }
                        Err(e) => {
                            eprintln!("SR: Failed to bind state read socket: {:?}. Retrying in 1s.", e);
                            thread::sleep(Duration::from_secs(1));
                            continue;
                        }
                    }
                }
                
                if let Some(s) = &socket {
                     match s.recv_from(&mut read_buf) {
                        Ok((len, addr)) => {
                            if let Ok(state) = XrState::deserialize_bin(&read_buf[0..len]) {
                                if let Some(p) = active_peers.iter_mut().find(|v| v.addr == addr) {
                                    p.last_seen = Instant::now();
                                    is_sr.send(XrNetIncoming::Update { peer: XrNetPeer { addr }, state }).ok();
                                } else {
                                    active_peers.push(ActivePeer { addr, last_seen: Instant::now() });
                                    is_sr.send(XrNetIncoming::Join { peer: XrNetPeer { addr }, state }).ok();
                                }
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => { /* Expected */ }
                        Err(e) => {
                            eprintln!("SR: Recv error: {:?}. Re-creating socket.", e);
                            socket = None; 
                            continue; 
                        }
                    }
                } else {
                     thread::sleep(Duration::from_millis(100)); // Socket is None, brief pause
                }

                let mut i = 0;
                while i < active_peers.len() {
                    if active_peers[i].last_seen.elapsed().as_secs_f64() > PEER_LEAVE_TIMEOUT {
                        let removed_peer_addr = active_peers.remove(i).addr;
                        let peer = XrNetPeer { addr: removed_peer_addr };
                        is_sr.send(XrNetIncoming::Leave { peer }).ok();
                        os_sr_leave.send(XrNetOutgoing::Leave(peer)).ok();
                    } else {
                        i += 1;
                    }
                }
            }
        });
        
        let state_write_thread = std::thread::spawn(move || {
            let mut socket: Option<UdpSocket> = None;
            let mut peers: Vec<XrNetPeer> = Default::default();
            while let Ok(msg) = outgoing_receiver.recv() {
                match msg{
                    XrNetOutgoing::Discovered(peer)=>{
                        if peers.iter_mut().find(|v| v.addr == peer.addr).is_none(){
                            peers.push(peer);
                        }
                    }
                    XrNetOutgoing::Leave(peer)=>{
                        peers.retain(|v| v.addr != peer.addr);
                    }
                    XrNetOutgoing::State(state)=>{ 
                        if socket.is_none() {
                            match UdpSocket::bind("0.0.0.0:0") {
                                Ok(s) => {
                                    socket = Some(s);
                                }
                                Err(e) => {
                                    eprintln!("SW: Failed to bind state write socket: {:?}. Send skipped, retrying later.", e);
                                    thread::sleep(Duration::from_secs(1)); 
                                    continue; 
                                }
                            }
                        }

                        if let Some(s) = &socket {
                            let buf = state.serialize_bin(); // Assuming this returns Vec<u8> directly
                            for peer_target in &peers{ // Renamed to avoid conflict with XrNetPeer struct
                                if let Err(e) = s.send_to(&buf, peer_target.addr){ // Use peer_target.addr
                                    eprintln!("SW: Send error to {:?}: {:?}.", peer_target.addr, e);
                                     // Check if error is non-recoverable for the socket itself
                                    if e.kind() != io::ErrorKind::WouldBlock && e.kind() != io::ErrorKind::TimedOut {
                                        socket = None; // Force re-creation on next State message
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    XrNetOutgoing::Break=>{
                        break;
                    }
                }
            }
        });
        
        Self{
            thread_loop,
            discovery_write_thread: Some(discovery_write_thread),
            discovery_read_thread: Some(discovery_read_thread),
            state_read_thread: Some(state_read_thread),
            state_write_thread: Some(state_write_thread),
            incoming_receiver: incoming_receiver_val, // Use the renamed variable
            outgoing_sender,
        }
    }
}
