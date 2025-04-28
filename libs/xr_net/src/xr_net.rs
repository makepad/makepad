
// ok so everyone sends interaction events to the system
// and the system has physics 'state' as output. 
// so how do we do this
use{
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
        self.outgoing_sender.send(XrNetOutgoing::Break).ok();
        self.discovery_write_thread.take().map(|v| v.join());
        self.discovery_read_thread.take().map(|v| v.join());
        self.state_read_thread.take().map(|v| v.join());
        self.state_write_thread.take().map(|v| v.join());
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
        // the UDP broadcast socket
        let discovery_bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), DISCOVERY_PORT);
        
        let discovery_send = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)), DISCOVERY_PORT);
        
        let read_discovery_socket = UdpSocket::bind(discovery_bind).unwrap();
        read_discovery_socket.set_read_timeout(Some(Duration::from_secs_f64(0.25))).unwrap();
        read_discovery_socket.set_broadcast(true).unwrap();
        let discovery_socket = read_discovery_socket.try_clone().unwrap();
        
        let my_client_uid = LiveId::from_str(&format!("{:?}", std::time::SystemTime::now())).0;
        
        // write thread
        let write_discovery_socket = discovery_socket.try_clone().unwrap();
        let thread_loop = Arc::new(AtomicBool::new(true));
        let thread_loop_2 = thread_loop.clone();
        let discovery_write_thread = std::thread::spawn(move || {
            while thread_loop_2.load(Ordering::Relaxed){
                write_discovery_socket.send_to(&my_client_uid.to_be_bytes(), discovery_send).ok();
                std::thread::sleep(Duration::from_secs_f64(0.1));
            }
        });

        let (outgoing_sender, outgoing_receiver) = mpsc::channel();
        let outgoing_sender2 = outgoing_sender.clone();
        let thread_loop_2 = thread_loop.clone();
        let discovery_read_thread = std::thread::spawn(move || {
            let mut read_buf = [0u8; 4096];
            // if we havent heard from a headset in 1 second we remove it from peers
            while thread_loop_2.load(Ordering::Relaxed){
                if let Ok((len, mut addr)) = read_discovery_socket.recv_from(&mut read_buf) {
                    addr.set_port(STATE_PORT);
                    let compare = my_client_uid.to_be_bytes();
                    if len == compare.len() && read_buf[0..compare.len()] != compare{
                        let peer = XrNetPeer{
                            addr,
                        };
                        outgoing_sender2.send(XrNetOutgoing::Discovered(peer)).ok();
                    }
                }
            }
        });
        
        
        let (incoming_sender, incoming_receiver) = mpsc::channel();
                
        // the xr state socket
        let state_bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), STATE_PORT);
        let read_state_socket = UdpSocket::bind(state_bind).unwrap();
        read_state_socket.set_read_timeout(Some(Duration::from_secs_f64(0.25))).unwrap();
        let write_state_socket = read_state_socket.try_clone().unwrap();
        // xr state receiver thread
        let outgoing_sender2 = outgoing_sender.clone();
        let thread_loop_2 = thread_loop.clone();
        let state_read_thread = std::thread::spawn(move || {
            // here we receive the XrState packets
            let mut read_buf = [0u8; 4096];
            pub struct Peer{
                addr: SocketAddr,
                last_seen: Instant,
            }
            let mut peers:Vec<Peer> = Vec::new();
            while thread_loop_2.load(Ordering::Relaxed){
                while let Ok((len, addr)) = read_state_socket.recv_from(&mut read_buf) {
                    // parse buffer
                    if let Ok(state) = XrState::deserialize_bin(&read_buf[0..len]){
                        // we got an xr state message, chuck it on the incoming stream
                        if let Some(peer) = peers.iter_mut().find(|v| v.addr == addr){
                            peer.last_seen = Instant::now();
                            incoming_sender.send(
                                XrNetIncoming::Update{
                                    peer: XrNetPeer{addr},
                                    state
                                }
                            ).ok();
                        }
                        else{
                            peers.push(Peer{addr, last_seen:Instant::now()});
                            incoming_sender.send(
                                XrNetIncoming::Join{
                                    peer: XrNetPeer{addr},
                                    state
                                }
                            ).ok();
                        }
                    }
                    let mut i = 0;
                    while i < peers.len(){
                        if peers[i].last_seen.elapsed().as_secs_f64()>PEER_LEAVE_TIMEOUT{
                            let peer = XrNetPeer{addr:peers.remove(i).addr};
                            incoming_sender.send(
                                XrNetIncoming::Leave{
                                    peer
                                }
                            ).ok();
                            outgoing_sender2.send(XrNetOutgoing::Leave(peer)).ok();
                        }
                        else{
                            i+=1;
                        }
                    }
                }
            }
        });
        
        // xr state output thread
        let state_write_thread = std::thread::spawn(move || {
            // here we receive the XrState packets
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
                    XrNetOutgoing::State(state)=>{ // send to all peers
                        let buf = state.serialize_bin();
                        for peer in &peers{
                            write_state_socket.send_to(&buf, peer.addr).unwrap();
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
            incoming_receiver,
            outgoing_sender,
        }
    }
}
