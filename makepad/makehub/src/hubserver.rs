use std::net::{TcpListener};
use std::sync::{mpsc, Arc, Mutex};
use crate::hubmessage::*;
use crate::hubclient::*;

pub struct HubServerConnection {
    _peer_addr: HubAddr,
    _read_thread: std::thread::JoinHandle<()>,
    _write_thread: std::thread::JoinHandle<()>,
    //write_thread: Option<std::thread::JoinHandle<()>>,
    _tx_write: mpsc::Sender<HubToClientMsg>
}

pub struct HubServer {
    pub listen_thread: Option<std::thread::JoinHandle<()>>,
    pub pump_thread: Option<std::thread::JoinHandle<()>>,
}

impl HubServer {
    pub fn start_hub_server() -> HubServer {
        // bind to all interfaces
        let listener = TcpListener::bind("0.0.0.0:51234").expect("Cannot bind server to 51243");
        
        //let (_tx_listen, _rx_listen) = mpsc::channel::<HubMessage>();
        let (tx_pump, rx_pump) = mpsc::channel::<(HubAddr, ClientToHubMsg)>();
        
        let connections = Arc::new(Mutex::new(Vec::<HubServerConnection>::new()));
        
        let listen_connections = Arc::clone(&connections);
        let listen_thread = std::thread::spawn(move || {
            
            for tcp_stream in listener.incoming() {
                let mut read_tcp_stream = tcp_stream.expect("Incoming stream failure");
                let mut write_tcp_stream = read_tcp_stream.try_clone().expect("Cannot clone tcp stream");
                let peer_addr = HubAddr::from_socket_addr(read_tcp_stream.peer_addr().expect("No peer address"));
                // clone our transmit-to-pump
                let tx_pump = tx_pump.clone();
                let read_peer_addr = peer_addr.clone();
                let read_thread = std::thread::spawn(move || {
                    loop {
                        let msg_buf = read_block_from_tcp_stream(&mut read_tcp_stream);
                        let cth_msg: ClientToHubMsg = bincode::deserialize(&msg_buf).expect("read_thread hub message deserialize fail");
                        tx_pump.send((read_peer_addr.clone(), cth_msg)).expect("tx_pump.send fails");
                    }
                });
                
                let (tx_write, rx_write) = mpsc::channel::<HubToClientMsg>();
                let write_thread = std::thread::spawn(move || {
                    while let Ok(htc_msg) = rx_write.recv() {
                        let msg_buf = bincode::serialize(&htc_msg).expect("write_thread hub message serialize fail");
                        write_block_to_tcp_stream(&mut write_tcp_stream, msg_buf);
                    }
                });
                
                if let Ok(mut connections) = listen_connections.lock() {
                    connections.push(HubServerConnection {
                        _peer_addr: peer_addr.clone(),
                        _read_thread: read_thread,
                        _write_thread: write_thread,
                        _tx_write: tx_write
                    })
                };
            }
        });
        let _pump_connections = Arc::clone(&connections);
        let pump_thread = std::thread::spawn(move || {
            // ok we get inbound messages from the threads
            while let Ok((from_addr, cth_msg)) = rx_pump.recv() {
                println!("Pump thread got message {:?} {:?}", from_addr, cth_msg);
                // we got a message.. now lets route it elsewhere
                // call our log closure with a message.
                
            }
        });
        
        return HubServer {listen_thread: Some(listen_thread), pump_thread: Some(pump_thread)};
    }

    pub fn join_threads(&mut self) {
        self.listen_thread.take().expect("cant take listen thread").join().expect("cant join listen");
        self.pump_thread.take().expect("cant take pump thread").join().expect("cant join pump");
    }

}
