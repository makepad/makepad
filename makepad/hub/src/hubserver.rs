use std::net::{TcpListener, TcpStream, SocketAddr, Shutdown};
use std::sync::{mpsc, Arc, Mutex};

use crate::hubmsg::*;
use crate::hubclient::*;
use crate::hubrouter::*;
use makepad_tinyserde::*;

#[derive(Debug, Clone, SerBin, DeBin, SerRon, DeRon, PartialEq)]
pub enum HubServerConfig {
    Offline, // no network connectivity
    Network(u16), // 0.0.0.0:port
    Localhost(u16), // 127.0.0.1:port
    InterfaceV4(u16, [u8; 4])
}

pub struct HubServerShared {
    pub terminate: bool,
    pub connections: Vec<(HubAddr, TcpStream)>
}

pub struct HubServer {
    pub shared: Arc<Mutex<HubServerShared>>,
    pub listen_address: Option<SocketAddr>,
    pub listen_thread: Option<std::thread::JoinHandle<()>>
}

impl HubServer {
    pub fn start_hub_server(digest: Digest, config: &HubServerConfig, hub_router: &HubRouter) -> Option<HubServer> {
        
        let listen_address = match config {
            HubServerConfig::Offline => return None,
            HubServerConfig::Localhost(port) => SocketAddr::from(([127, 0, 0, 1], *port)),
            HubServerConfig::Network(port) => SocketAddr::from(([0, 0, 0, 0], *port)),
            HubServerConfig::InterfaceV4(port, ip) => SocketAddr::from((*ip, *port)),
        };
        
        let listener = if let Ok(listener) = TcpListener::bind(listen_address) {listener}else {println!("start_hub_server bind server address"); return None};
        let listen_address = listener.local_addr().expect("Cannot get local address");
        
        let tx_pump = hub_router.tx_pump.clone();
        let routes = Arc::clone(&hub_router.routes); //Arc::new(Mutex::new(Vec::<HubServerConnection>::new()));
        let shared = Arc::new(Mutex::new(HubServerShared {
            connections: Vec::new(),
            terminate: false
        }));
        
        let listen_thread = {
            //let hub_log = hub_log.clone();
            let routes = Arc::clone(&routes);
            let shared = Arc::clone(&shared);
            let digest = digest.clone();
            std::thread::spawn(move || {
                for tcp_stream in listener.incoming() {
                    let tcp_stream = tcp_stream.expect("Incoming stream failure");
                    let peer_addr = HubAddr::from_socket_addr(tcp_stream.peer_addr().expect("No peer address"));
                    
                    if let Ok(mut shared) = shared.lock() {
                        if shared.terminate {
                            for (_, tcp_stream) in &mut shared.connections {
                                let _ = tcp_stream.shutdown(Shutdown::Both);
                            }
                            // lets disconnect all our connections
                            return
                        }
                        let tcp_stream = tcp_stream.try_clone().expect("Cannot clone tcp stream");
                        shared.connections.push((peer_addr, tcp_stream));
                    }
                    
                    let (tx_write, rx_write) = mpsc::channel::<FromHubMsg>();
                    let tx_write_copy = tx_write.clone();
                    // clone our transmit-to-pump
                    let _read_thread = {
                        let tx_pump = tx_pump.clone();
                        let digest = digest.clone();
                        let peer_addr = peer_addr.clone();
                        let mut tcp_stream = tcp_stream.try_clone().expect("Cannot clone tcp stream");
                        //let hub_log = hub_log.clone();
                        std::thread::spawn(move || {
                            loop {
                                match read_block_from_tcp_stream(&mut tcp_stream, digest.clone()) {
                                    Ok(msg_buf) => {
                                        let cth_msg: ToHubMsg = DeBin::deserialize_bin(&msg_buf).expect("Can't parse binary");
                                        tx_pump.send((peer_addr.clone(), cth_msg)).expect("tx_pump.send fails - should never happen");
                                    }
                                    Err(e) => {
                                        let _ = tcp_stream.shutdown(Shutdown::Both);
                                        let _ = tx_pump.send((peer_addr.clone(), ToHubMsg {
                                            to: HubMsgTo::Hub,
                                            msg: HubMsg::ConnectionError(e.clone())
                                        })).expect("tx_pump.send fails - should never happen");
                                        // lets break rx write
                                        let _ = tx_write_copy.send(FromHubMsg {
                                            from: peer_addr.clone(),
                                            msg: HubMsg::ConnectionError(e)
                                        });
                                        return
                                    }
                                }
                            }
                        })
                    };
                    let _write_thread = {
                        let digest = digest.clone();
                        let peer_addr = peer_addr.clone();
                        let tx_pump = tx_pump.clone();
                        let shared = Arc::clone(&shared);
                        let mut tcp_stream = tcp_stream.try_clone().expect("Cannot clone tcp stream");
                        //let hub_log = hub_log.clone();
                        std::thread::spawn(move || {
                            while let Ok(htc_msg) = rx_write.recv() {
                                match &htc_msg.msg {
                                    HubMsg::ConnectionError(_) => { // we are closed by the read loop
                                        let _ = tcp_stream.shutdown(Shutdown::Both);
                                        break
                                    },
                                    _ => ()
                                }
                                let mut msg_buf = Vec::new(); 
                                htc_msg.ser_bin(&mut msg_buf);
                                
                                if let Err(e) = write_block_to_tcp_stream(&mut tcp_stream, &msg_buf, digest.clone()) {
                                    // disconnect the socket and send shutdown
                                    let _ = tcp_stream.shutdown(Shutdown::Both);
                                    tx_pump.send((peer_addr.clone(), ToHubMsg {
                                        to: HubMsgTo::Hub,
                                        msg: HubMsg::ConnectionError(e)
                                    })).expect("tx_pump.send fails - should never happen");
                                }
                            }
                            // remove tx_write from our shared pool
                            if let Ok(mut shared) = shared.lock() {
                                while let Some(position) = shared.connections.iter().position( | (addr, _) | *addr == peer_addr) {
                                    shared.connections.remove(position);
                                }
                            }
                        })
                    };
                    
                    if let Ok(mut routes) = routes.lock() {
                        routes.push(HubRoute {
                            route_type: HubRouteType::Unknown,
                            peer_addr: peer_addr.clone(),
                            tcp_stream: Some(tcp_stream),
                            tx_write: tx_write
                        })
                    };
                }
            })
        };
        
        let hub_server = HubServer {
            shared: shared,
            listen_address: Some(listen_address),
            listen_thread: Some(listen_thread),
        };
        
        
        return Some(hub_server);
    }
    
    pub fn terminate(&mut self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.terminate = true;
        }
        if let Some(listen_address) = self.listen_address {
            self.listen_address = None;
            // just do a single connection to the listen address to break the wait.
            if let Ok(_) = TcpStream::connect(listen_address) {
                self.listen_thread.take().expect("cant take listen thread").join().expect("cant join listen thread");
            }
        }
    }
}
