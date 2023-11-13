use crate::os::OsWebSocket;
use crate::event::HttpRequest;
use std::sync::mpsc::{TryRecvError,RecvError};

pub trait CxWebSocketsApi {
    fn websocket_open(&mut self, request: HttpRequest)->WebSocket;
}

pub struct WebSocket{
    pub os: OsWebSocket
}

pub enum WebSocketMessage{
    Error(String),
    Binary(Vec<u8>),
    String(String)
}

impl WebSocket{    
    pub fn open(request:HttpRequest)->WebSocket{
        WebSocket{
            os:OsWebSocket::open(request)
        }
    }
    
    pub fn send_binary(&mut self, data:&[u8])->Result<(),()>{
        self.os.send_binary(data)
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