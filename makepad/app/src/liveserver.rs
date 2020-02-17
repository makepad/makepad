// live value connection server
use makepad_tinyserde::*;

#[derive(Debug, Clone, SerRon, DeRon, PartialEq)]
pub enum LiveServerConfig {
    Offline, // no network connectivity
    Network(u16), // 0.0.0.0:port
    Localhost(u16), // 127.0.0.1:port
    InterfaceV4(u16, [u8; 4])
}

enum LiveMsg{
    
}

struct LiveServer {
    pub fn start_live_server(config: &LiveServerConfig) -> Option<LiveServer> {
        let listen_address = match config {
            LiveServerConfig::Offline => return None,
            LiveServerConfig::Localhost(port) => SocketAddr::from(([127, 0, 0, 1], *port)),
            LiveServerConfig::Network(port) => SocketAddr::from(([0, 0, 0, 0], *port)),
            LiveServerConfig::InterfaceV4(port, ip) => SocketAddr::from((*ip, *port)),
        };
        
        let listener = if let Ok(listener) = TcpListener::bind(listen_address) {
            listener
        }
        else {
            println!("start_live_server bind server address");
            return None
        };
        let listen_address = listener.local_addr().expect("Cannot get local address");
        
        let listen_thread = {
            //let hub_log = hub_log.clone();
            std::thread::spawn(move || {
                for tcp_stream in listener.incoming() {
                    let tcp_stream = tcp_stream.expect("Incoming stream failure");
                    let peer_addr = tcp_stream.peer_addr().expect("No peer address");
                    
                    
                    
                }
            })
        };
    }
}