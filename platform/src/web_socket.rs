use crate::os::OsWebSocket;
use crate::event::HttpRequest;
use std::sync::{
    mpsc::{channel, Sender, Receiver, TryRecvError,RecvError}
};

pub struct WebSocket{
    pub tx_sender: Sender<WebSocketMessage>,
    pub rx_receiver: Receiver<WebSocketMessage>,
    pub os: OsWebSocket
}

pub enum WebSocketMessage{
    Error(String),
    Binary(Vec<u8>),
    String(String),
    Closed
}

impl WebSocket{    
    pub fn open(request:HttpRequest)->WebSocket {
        let (tx_sender, _tx_receiver) = channel();
        let (_rx_sender, rx_receiver) = channel();
        WebSocket{
            tx_sender,
            rx_receiver,
            os:OsWebSocket::open(request)
        }
    }
    
    pub fn send_binary(&mut self, data:Vec<u8>)->Result<(),()>{
        self.os.send_binary(&data)
    }
    
    pub fn send_string(&mut self, data:&str)->Result<(),()>{
        self.os.send_string(data)
    }
    
    pub fn try_recv(&mut self)->Result<WebSocketMessage,TryRecvError>{
        self.os.try_recv()
    }
    
    pub fn recv(&mut self)->Result<WebSocketMessage,RecvError>{
        self.os.recv()
    }
}
