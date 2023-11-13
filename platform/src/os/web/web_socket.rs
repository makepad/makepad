
use crate::event::HttpRequest;
use crate::web_socket::{WebSocketMessage};
use std::sync::mpsc::{TryRecvError, RecvError};
pub struct OsWebSocket{
}

impl OsWebSocket{
    pub fn try_recv(&mut self)->Result<WebSocketMessage,TryRecvError>{
        todo!()
    }
            
    pub fn recv(&mut self)->Result<WebSocketMessage,RecvError>{
        todo!()
    }
            
    pub fn send_binary(&mut self, _data:&[u8])->Result<(),()>{
        todo!()
    }
                
    pub fn send_string(&mut self, _data:&str)->Result<(),()>{
        todo!()
    }
            
    pub fn open(_request: HttpRequest)->OsWebSocket{
        todo!()
    }
}