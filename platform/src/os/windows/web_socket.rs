
use crate::event::HttpRequest;
use crate::web_socket::{WebSocketMessage};
use std::sync::mpsc::{Sender,};
pub struct OsWebSocket{
}

impl OsWebSocket{
    pub fn send_message(&mut self, _message:WebSocketMessage)->Result<(),()>{
        todo!("Websockets not implemented yet on windows, we'll get there");
    }
                        
    pub fn open(_request: HttpRequest, _rx_sender:Sender<WebSocketMessage>)->OsWebSocket{
        todo!("Websockets not implemented yet on windows, we'll get there");
    }
}