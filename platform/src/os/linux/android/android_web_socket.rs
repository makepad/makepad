
use crate::event::HttpRequest;
use crate::web_socket::WebSocketMessage;
use std::sync::Arc;
use std::sync::mpsc::{self, *};
use self::super::android_jni;
use crate::LiveId;
use makepad_http::websocket::{MessageHeader, MessageFormat, WebSocket};

pub struct OsWebSocket{
    pub recv: Receiver<WebSocketMessage>,
    pub sender_ref: Arc<Box<Sender<WebSocketMessage>>>,
    pub request_id: LiveId,
}

pub type WebsocketIncomingMessageFn = Box<dyn FnMut(WebSocketMessage) + Send  + 'static>;

impl OsWebSocket{
    pub fn try_recv(&mut self)->Result<WebSocketMessage,TryRecvError>{
        self.recv.try_recv()
    }
            
    pub fn recv(&mut self)->Result<WebSocketMessage,RecvError>{
        self.recv.recv()
    }
            
    pub fn send_binary(&mut self, data:&[u8])->Result<(),()>{
        let header = MessageHeader::from_len(data.len(), MessageFormat::Text, true);
        let frame = WebSocket::build_message(header, &data);

        unsafe {android_jni::to_java_websocket_send_message(self.request_id, frame);}

        Ok(())
    }
                
    pub fn send_string(&mut self, data:&str)->Result<(),()>{
        let header = MessageHeader::from_len(data.len(), MessageFormat::Text, true);
        let frame = WebSocket::build_message(header, &data.to_string().into_bytes());

        unsafe {android_jni::to_java_websocket_send_message(self.request_id, frame);}

        Ok(())
    }
            
    pub fn open(request: HttpRequest)->OsWebSocket{
        let request_id = LiveId::unique();
        let (sender,recv) = mpsc::channel();

        let sender_ref = Arc::new(Box::new(sender));
        let pointer = Arc::as_ptr(&sender_ref);

        unsafe {android_jni::to_java_websocket_open(request_id, request, pointer);}

        OsWebSocket{
            recv,
            sender_ref,
            request_id
        }
    }
}