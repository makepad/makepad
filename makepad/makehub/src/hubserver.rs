use std::net::{TcpStream, TcpListener, UdpSocket, SocketAddr, Shutdown};
use std::sync::{mpsc, Arc, Mutex};
use std::{time, thread};
use crate::hubmsg::*;
use crate::hubclient::*;

#[derive(PartialEq)]
pub enum HubClientType{
    Unknown,
    Build,
    UI
}

pub struct HubServerConnection {
    peer_addr: HubAddr,
    tx_write: mpsc::Sender<HubToClientMsg>,
    tcp_stream: TcpStream,
    client_type: HubClientType
}

pub struct HubServer {
    pub listen_port: u16,
    pub listen_thread: Option<std::thread::JoinHandle<()>>,
    pub router_thread: Option<std::thread::JoinHandle<()>>,
    pub announce_thread: Option<std::thread::JoinHandle<()>>,
}

impl HubServer {
    pub fn start_hub_server_default(key: &[u8])->HubServer{
         HubServer::start_hub_server(
            &key,
            SocketAddr::from(([0, 0, 0, 0], 0))
        )
    }
    
    pub fn start_hub_server(key: &[u8], listen_address: SocketAddr) -> HubServer {
        
        let listener = TcpListener::bind(listen_address).expect("Cannot bind server address");
        let listen_port = listener.local_addr().expect("Cannot get server local address").port();
        
        let (tx_pump, rx_pump) = mpsc::channel::<(HubAddr, ClientToHubMsg)>();
        
        let connections = Arc::new(Mutex::new(Vec::<HubServerConnection>::new()));
        
        let mut digest = [0u64; 26];
        digest_buffer(&mut digest, key);
        
        let listen_thread = {
            let connections = Arc::clone(&connections);
            std::thread::spawn(move || {
                
                for tcp_stream in listener.incoming() {
                    let tcp_stream = tcp_stream.expect("Incoming stream failure");
                    let peer_addr = HubAddr::from_socket_addr(tcp_stream.peer_addr().expect("No peer address"));
                    // clone our transmit-to-pump
                    let _read_thread = {
                        let tx_pump = tx_pump.clone();
                        let peer_addr = peer_addr.clone();
                        let digest = digest.clone();
                        let peer_addr = peer_addr.clone();
                        let mut tcp_stream = tcp_stream.try_clone().expect("Cannot clone tcp stream");
                        std::thread::spawn(move || {
                            loop {
                                match read_block_from_tcp_stream(&mut tcp_stream, &mut digest.clone()) {
                                    Ok(msg_buf) => {
                                        let cth_msg: ClientToHubMsg = bincode::deserialize(&msg_buf).expect("read_thread hub message deserialize fail - should never happen");
                                        tx_pump.send((peer_addr.clone(), cth_msg)).expect("tx_pump.send fails - should never happen");
                                    }
                                    Err(e) => {
                                        if tcp_stream.shutdown(Shutdown::Both).is_ok() {
                                            tx_pump.send((peer_addr.clone(), ClientToHubMsg {
                                                to: HubMsgTo::Hub,
                                                msg: HubMsg::ConnectionError(e)
                                            })).expect("tx_pump.send fails - should never happen");
                                        }
                                        return
                                    }
                                }
                            }
                        })
                    };
                    let (tx_write, rx_write) = mpsc::channel::<HubToClientMsg>();
                    let _write_thread = {
                        let digest = digest.clone();
                        let peer_addr = peer_addr.clone();
                        let tx_pump = tx_pump.clone();
                        let mut tcp_stream = tcp_stream.try_clone().expect("Cannot clone tcp stream");
                        std::thread::spawn(move || {
                            while let Ok(htc_msg) = rx_write.recv() {
                                let msg_buf = bincode::serialize(&htc_msg).expect("write_thread hub message serialize fail");
                                if let Err(e) = write_block_to_tcp_stream(&mut tcp_stream, &msg_buf, &mut digest.clone()) {
                                    // disconnect the socket and send shutdown
                                    if tcp_stream.shutdown(Shutdown::Both).is_ok() {
                                        tx_pump.send((peer_addr.clone(), ClientToHubMsg {
                                            to: HubMsgTo::Hub,
                                            msg: HubMsg::ConnectionError(e)
                                        })).expect("tx_pump.send fails - should never happen");
                                    }
                                }
                            }
                        })
                    };
                    
                    if let Ok(mut connections) = connections.lock() {
                        connections.push(HubServerConnection {
                            client_type: HubClientType::Unknown,
                            peer_addr: peer_addr.clone(),
                            tcp_stream: tcp_stream,
                            tx_write: tx_write
                        })
                    };
                }
            })
        };
        
        let router_thread = {
            let connections = Arc::clone(&connections);
            std::thread::spawn(move || {
                // ok we get inbound messages from the threads
                while let Ok((from, cth_msg)) = rx_pump.recv() {
                    let to = cth_msg.to;
                    let htc_msg = HubToClientMsg {
                        from: from,
                        msg: cth_msg.msg
                    };
                    // we got a message.. now lets route it elsewhere
                    if let Ok(mut connections) = connections.lock() {

                        println!("Router thread got message {:?}", htc_msg);
                        
                        if let Some(cid) = connections.iter().position( | c | c.peer_addr == htc_msg.from) {
                            if connections[cid].client_type == HubClientType::Unknown {
                                match &htc_msg.msg {
                                    HubMsg::ConnectBuild => { // send it to all clients
                                        connections[cid].client_type = HubClientType::Build;
                                    },
                                    HubMsg::ConnectUI => { // send it to all clients
                                        connections[cid].client_type = HubClientType::UI;
                                    },
                                    _ => {
                                        println!("Router got message from unknown client {:?}, disconnecting", htc_msg.from);
                                        let _ = connections[cid].tcp_stream.shutdown(Shutdown::Both);
                                        connections.remove(cid);
                                        continue;
                                    }
                                }
                            }
                        }
                        
                        match to {
                            HubMsgTo::All => { // send it to all
                                for connection in connections.iter() {
                                    if connection.client_type != HubClientType::Unknown {
                                        connection.tx_write.send(htc_msg.clone()).expect("Could not tx_write.send");
                                    }
                                }
                            },
                            HubMsgTo::Client(addr) => { // find our specific addr and send
                                if let Some(connection) = connections.iter().find( | c | c.peer_addr == addr) {
                                    if connection.client_type != HubClientType::Unknown {
                                        connection.tx_write.send(htc_msg).expect("Could not tx_write.send");
                                        break;
                                    }
                                }
                            },
                            HubMsgTo::Build=>{
                                for connection in connections.iter() {
                                    if connection.client_type == HubClientType::Build{
                                        connection.tx_write.send(htc_msg).expect("Could not tx_write.send");
                                        break;
                                    }
                                }
                            },
                            HubMsgTo::UI=>{
                                for connection in connections.iter() {
                                    if connection.client_type == HubClientType::UI{
                                        connection.tx_write.send(htc_msg).expect("Could not tx_write.send");
                                        break;
                                    }
                                }
                            },
                            HubMsgTo::Hub => { // process queries on the hub
                                match &htc_msg.msg {
                                    HubMsg::ConnectionError(e) => {
                                        // connection error, lets remove connection
                                        if let Some(pos) = connections.iter().position( | c | c.peer_addr == htc_msg.from) {
                                            println!("Server closing connection {:?} from error {:?}", htc_msg.from, e);
                                            connections.remove(pos);
                                        }
                                    },
                                    _ => ()
                                }
                                // return current connections
                            }
                        }
                    }
                }
            })
        };
        
        return HubServer {
            listen_port: listen_port,
            listen_thread: Some(listen_thread),
            router_thread: Some(router_thread),
            announce_thread: None
        };
    }

    pub fn start_announce_server_default(&mut self, key: &[u8]) {
        self.start_announce_server(
            &key,
            SocketAddr::from(([0, 0, 0, 0], 0)),
            SocketAddr::from(([255, 255, 255, 255], HUB_ANNOUNCE_PORT)),
            SocketAddr::from(([127, 0, 0, 1], HUB_ANNOUNCE_PORT)),
        )
}
    
    pub fn start_announce_server(&mut self, key: &[u8], announce_bind: SocketAddr, announce_send: SocketAddr, announce_backup: SocketAddr) {
        let listen_port = self.listen_port;
        
        let mut digest = [0u64; 26];
        digest[25] = listen_port as u64;
        digest[0] = listen_port as u64;
        digest_buffer(&mut digest, key);
        
        let digest_u8 = unsafe {std::mem::transmute::<[u64; 26], [u8; 26 * 8]>(digest)};
        
        let announce_thread = std::thread::spawn(move || {
            let socket = UdpSocket::bind(announce_bind).expect("Server: Cannot bind announce port");
            socket.set_broadcast(true).expect("Server: cannot set broadcast on announce ip");
            
            let thread_sleep_time = time::Duration::from_millis(100);
            loop {
                if let Err(_) = socket.send_to(&digest_u8, announce_send){
                    if let Err(_) = socket.send_to(&digest_u8, announce_backup){
                        println!("Cannot send to announce port");
                        return
                    }
                }
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
