
use crate::event::HttpRequest;
use crate::web_socket::{WebSocketMessage};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{self, *};
use std::collections::HashMap;
use self::super::android_jni;
use crate::LiveId;

pub struct OsWebSocket{
    pub recv: Receiver<WebSocketMessage>,
    pub sender: Sender<WebSocketMessage>,
    pub request_id: LiveId,
}

impl OsWebSocket{
    pub fn try_recv(&mut self)->Result<WebSocketMessage,TryRecvError>{
        self.recv.try_recv()
    }
            
    pub fn recv(&mut self)->Result<WebSocketMessage,RecvError>{
        self.recv.recv()
    }
            
    pub fn send_binary(&mut self, _data:&[u8])->Result<(),()>{
        todo!()
    }
                
    pub fn send_string(&mut self, _data:&str)->Result<(),()>{
        todo!()
    }
            
    pub fn open(request: HttpRequest)->OsWebSocket{
        let request_id = LiveId::unique();
        unsafe {android_jni::to_java_websocket_open(request_id, request);}

        let (sender,recv) = mpsc::channel();

        OsWebSocket{
            recv,
            sender,
            request_id
        }
    }
}

#[derive(Default)]
pub struct CxWebSockets {
    pub (crate) active_websocket_senders: HashMap<LiveId,Arc<Mutex<Sender<WebSocketMessage>>>>
}