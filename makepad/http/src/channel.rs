use std::net::{TcpStream, Shutdown};
use std::time::{Duration, Instant};
use std::sync::{Arc, Mutex, mpsc};
use std::collections::HashMap;
use crate::httputil::*; 
use crate::websocket::*;
use std::io::prelude::*;


#[derive(Clone, PartialEq, Eq, Hash)]
pub struct WebSocketId(pub u32);
pub struct WebSocketChannel {
    pub id_alloc: u32,
    pub tx_bus:  mpsc::Sender<(WebSocketId,Vec<u8>)>,
    pub bus_thread: std::thread::JoinHandle<()>,
    pub out_sockets: HashMap<WebSocketId, mpsc::Sender<Vec<u8>>>,
}

#[derive(Clone, Default)]
pub struct WebSocketChannels{
    channels: Arc<Mutex<HashMap<String,WebSocketChannel>>>
}

impl WebSocketChannels{
    pub fn send_directly(
        &self, 
        url: &str,
        data:Vec<u8>
    ){
        if let Ok(mut channels) = self.channels.lock(){
            if let Some(channel) = channels.get_mut(url){
                let socket_id = WebSocketId(0);
                let _ = channel.tx_bus.send((socket_id, data));
            }
        }
    }
    
    pub fn handle_websocket(
        self,
        tcp_stream: &mut TcpStream,
        url: &str,
        key: &str,
    ) {
        let upgrade_response = WebSocket::create_upgrade_response(key);

        write_bytes_to_tcp_stream_no_error(tcp_stream, upgrade_response.as_bytes());
        
        // start a thread for the write side
        let mut write_tcp_stream = tcp_stream.try_clone().unwrap();

        let (tx_socket, rx_socket) = mpsc::channel::<Vec<u8>>();
        
        let (socket_id, tx_bus) = self.add_socket_tx(url, tx_socket);
        
        // ok we get a socket id, we'll add that to our websocket datavec as a header.
        let mut web_socket = WebSocket::new();
        
        // start a write thread for the return messages
        let _write_thread = std::thread::spawn(move || {
            // we have a bus we read from, which we hand to our websocket server.
            while let Ok(data) = rx_socket.recv(){
                write_bytes_to_tcp_stream_no_error(&mut write_tcp_stream, &data);
            }
            let _ = write_tcp_stream.shutdown(Shutdown::Both);
        });
        
        // do the websocket read here
        loop {
            let mut data = [0u8; 1024];
            match tcp_stream.read(&mut data) {
                Ok(n) => {
                    if n == 0{
                        let _ = tcp_stream.shutdown(Shutdown::Both);
                        self.remove_socket(url, socket_id);
                        return
                    }
                    for result in web_socket.parse(&data[0..n]){
                        match result{
                            WebSocketResult::Ping(_)=>{},
                            WebSocketResult::Pong(_)=>{},
                            WebSocketResult::Data(data)=>{
                                // we have to send this to the websocket server
                                tx_bus.send((socket_id.clone(),data)).unwrap();
                                //let s = std::str::from_utf8(&data);
                            },
                            WebSocketResult::Error(_)=>{},
                            WebSocketResult::Close=>{
                            }
                        }
                    }
                }
                Err(_) => {
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                    self.remove_socket(url, socket_id);
                    return
                }
            }
        }
    }
    
    
    pub fn add_channel_if_none(&self, in_url:&str){
        // lets start a thread
        if let Ok(mut channels) = self.channels.lock(){
            if channels.get(in_url).is_some(){
                return
            }
            let (tx_bus, rx_bus) = mpsc::channel::<(WebSocketId,Vec<u8>)>();
            let arc_channels = self.channels.clone();
            let url = in_url.to_string();
            let thread = std::thread::spawn(move || {
                // we should collect all the messages we can in 50ms and then send it out together in 1 message
                let max_wait = Duration::from_millis(16);
                let mut last_start = Instant::now();
                let mut msg_stack = Vec::new();
                loop{
                    // do the message pump, but combine messages to not overflow the sockets
                    if let Ok((websocket_id,msg)) = rx_bus.recv_timeout(max_wait) {
                        msg_stack.push((websocket_id,msg));
                    }
                    if last_start.elapsed() > max_wait{ 
                        if msg_stack.len() > 0{ // send it out all together
                            // lets push out a little header: numclients, nummsgs
                            
                            let mut all_msg_len = 0;
                            for (_socket_id, msg) in &mut msg_stack{
                                all_msg_len += msg.len() + std::mem::size_of::<u32>();
                            }

                            let mut out_sockets = HashMap::new();
                            
                            // quickly lock sockets
                            if let Ok(channels) = arc_channels.lock(){
                                if let Some(channel) = channels.get(&url){
                                    out_sockets = channel.out_sockets.clone()
                                }
                            }
                            for (my_socket_id, tx_socket) in &out_sockets{
                                // lets build the message header
                                let mut head = Vec::new();
                                // number of sockets
                                head.extend_from_slice(&(out_sockets.len() as u64).to_le_bytes());
                                // my own socket
                                head.extend_from_slice(&(my_socket_id.0 as u32).to_le_bytes());
                                // all other sockets
                                for (socket_id, _) in &out_sockets{
                                    if socket_id != my_socket_id{
                                        head.extend_from_slice(&(socket_id.0 as u32).to_le_bytes());
                                    }
                                }
                                // the number of messages
                                head.extend_from_slice(&(msg_stack.len() as u64).to_le_bytes());
                                
                                let mut ws_msg  = WebSocketMessage::new_binary(head.len() + all_msg_len);
                                ws_msg.append(&head); 
                                
                                for (websocket_id,msg) in &mut msg_stack{
                                    ws_msg.append(&(websocket_id.0 as u32).to_le_bytes());
                                    ws_msg.append(msg);
                                }
                                
                                // send it off blindly
                                let _ = tx_socket.send(ws_msg.take());
                            }
                            
                            msg_stack.truncate(0);
                        }
                        last_start = Instant::now()
                    } 
                }
            });
            channels.insert(in_url.to_string(), WebSocketChannel{
                id_alloc: 1,
                out_sockets:HashMap::new(),
                tx_bus: tx_bus,
                bus_thread: thread
            });
        }
    }
    
    pub fn add_socket_tx(&self, url:&str, tx:mpsc::Sender::<Vec<u8>>)->(WebSocketId, mpsc::Sender<(WebSocketId,Vec<u8>)>){
        self.add_channel_if_none(url);
        
        if let Ok(mut channels) = self.channels.lock(){
            if let Some(channel) = channels.get_mut(url){
                let socket_id = WebSocketId(channel.id_alloc);
                channel.id_alloc += 1;
                channel.out_sockets.insert(socket_id.clone(), tx);
                return (socket_id, channel.tx_bus.clone())
            }
            else{
                panic!()
            }
        }
        else{
            panic!()
        }
    }
    
    pub fn remove_socket(&self, url:&str, socket_id:WebSocketId){
        if let Ok(mut servers) = self.channels.lock(){
            if let Some(server) = servers.get_mut(url){
                server.out_sockets.remove(&socket_id);
            }
        }
    }
}

