use std::net::{TcpListener, UdpSocket, SocketAddr};
use std::sync::{mpsc, Arc, Mutex};
use std::{time, thread};
use crate::hubmsg::*;
use crate::hubclient::*;

pub struct HubServerConnection {
    peer_addr: HubAddr,
    _read_thread: std::thread::JoinHandle<()>,
    _write_thread: std::thread::JoinHandle<()>,
    tx_write: mpsc::Sender<HubToClientMsg>
}

pub struct HubServer {
    pub listen_port: u16,
    pub listen_thread: Option<std::thread::JoinHandle<()>>,
    pub router_thread: Option<std::thread::JoinHandle<()>>,
    pub announce_thread: Option<std::thread::JoinHandle<()>>,
}

impl HubServer {
    pub fn start_hub_server(key: &[u8], listen_address: SocketAddr) -> HubServer {
        
        let listener = TcpListener::bind(listen_address).expect("Cannot bind server address");
        let listen_port = listener.local_addr().expect("Cannot get server local address").port();
        
        //let (_tx_listen, _rx_listen) = mpsc::channel::<HubMessage>();
        let (tx_pump, rx_pump) = mpsc::channel::<(HubAddr, ClientToHubMsg)>();
        
        let connections = Arc::new(Mutex::new(Vec::<HubServerConnection>::new()));

        let mut digest = [0u64; 26];
        digest_buffer(&mut digest, key);
        
        let listen_connections = Arc::clone(&connections);
        let listen_thread = std::thread::spawn(move || {
            
            for tcp_stream in listener.incoming() {
                let mut read_tcp_stream = tcp_stream.expect("Incoming stream failure");
                let mut write_tcp_stream = read_tcp_stream.try_clone().expect("Cannot clone tcp stream");
                let peer_addr = HubAddr::from_socket_addr(read_tcp_stream.peer_addr().expect("No peer address"));
                // clone our transmit-to-pump
                let tx_pump = tx_pump.clone();
                let read_peer_addr = peer_addr.clone();
                let read_digest = digest.clone();
                let read_thread = std::thread::spawn(move || {
                    loop {
                        let msg_buf_result = read_block_from_tcp_stream(&mut read_tcp_stream, &mut read_digest.clone());
                        if let Ok(msg_buf) = msg_buf_result{
                            let cth_msg: ClientToHubMsg = bincode::deserialize(&msg_buf).expect("read_thread hub message deserialize fail");
                            tx_pump.send((read_peer_addr.clone(), cth_msg)).expect("tx_pump.send fails");
                        }
                    }
                });
                let write_digest = digest.clone();
                let (tx_write, rx_write) = mpsc::channel::<HubToClientMsg>();
                let write_thread = std::thread::spawn(move || {
                    while let Ok(htc_msg) = rx_write.recv() {
                        let msg_buf = bincode::serialize(&htc_msg).expect("write_thread hub message serialize fail");
                        write_block_to_tcp_stream(&mut write_tcp_stream, &msg_buf, &mut write_digest.clone());
                    }
                });
                
                if let Ok(mut connections) = listen_connections.lock() {
                    connections.push(HubServerConnection {
                        peer_addr: peer_addr.clone(),
                        tx_write: tx_write,
                        _read_thread: read_thread,
                        _write_thread: write_thread
                    })
                };
            }
        });
        
        let router_connections = Arc::clone(&connections);
        let router_thread = std::thread::spawn(move || {
            // ok we get inbound messages from the threads
            while let Ok((from_addr, cth_msg)) = rx_pump.recv() {
                println!("Router thread got message {:?} {:?}", from_addr, cth_msg);
                
                let target = cth_msg.target;
                let htc_msg = HubToClientMsg {
                    from: from_addr,
                    msg: cth_msg.msg
                };
                // we got a message.. now lets route it elsewhere
                if let Ok(connections) = router_connections.lock() {
                    match target {
                        HubTarget::AllClients => { // send it to all
                            for connection in connections.iter() {
                                connection.tx_write.send(htc_msg.clone()).expect("Could not tx_write.send");
                            }
                        },
                        HubTarget::Client(hub_addr) => { // find our specific addr and send
                            if let Some(connection) = connections.iter().find( | c | c.peer_addr == hub_addr) {
                                connection.tx_write.send(htc_msg).expect("Could not tx_write.send");
                                break;
                            }
                        },
                        HubTarget::HubOnly => { // process queries on the hub
                            // return current connections
                        }
                    }
                }
            }
        });
        
        return HubServer {
            listen_port: listen_port,
            listen_thread: Some(listen_thread),
            router_thread: Some(router_thread),
            announce_thread: None
        };
    }
    
    pub fn start_announce_server(&mut self, key: &[u8], announce_bind: SocketAddr, announce_send: SocketAddr) {
        let listen_port = self.listen_port;
        
        let mut digest = [0u64; 26];
        digest[25] = listen_port as u64;
        digest[0] = listen_port as u64;
        digest_buffer(&mut digest, key);
        
        let digest_u8 = unsafe {std::mem::transmute::<[u64;26], [u8; 26*8]>(digest)};
        
        let announce_thread = std::thread::spawn(move || {
            let socket = UdpSocket::bind(announce_bind).expect("Server: Cannot bind announce port");
            socket.set_broadcast(true).expect("Server: cannot set broadcast on announce ip");
            
            let thread_sleep_time = time::Duration::from_millis(100);
            loop {
                socket.send_to(&digest_u8, announce_send).expect("Cannot write to announce port");
                thread::sleep(thread_sleep_time.clone());
            }
        });
        self.announce_thread = Some(announce_thread);
    }
    
    pub fn join_threads(&mut self) {
        self.listen_thread.take().expect("cant take listen thread").join().expect("cant join listen thread");
        self.router_thread.take().expect("cant take router thread").join().expect("cant join router thread");
        if self.announce_thread.is_some() {
            self.announce_thread.take().expect("cant take announce thread").join().expect("cant join announce_thread thread");
        }
    }
    
}
