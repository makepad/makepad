
use crate::event::HttpRequest;
use crate::web_socket::WebSocketMessage;
use std::sync::Arc;
use std::sync::mpsc::{*};
use self::super::android_jni;
use crate::LiveId;
use makepad_http::websocket::{MessageHeader, MessageFormat, WebSocket};

pub struct OsWebSocket{
    pub sender_ref: Arc<Box<Sender<WebSocketMessage>>>,
    pub request_id: LiveId,
}

pub type WebsocketIncomingMessageFn = Box<dyn FnMut(WebSocketMessage) + Send  + 'static>;

impl OsWebSocket{

    pub fn send_message(&mut self, message:WebSocketMessage)->Result<(),()>{
        let frame = match &message{
            WebSocketMessage::String(data)=>{
                let header = MessageHeader::from_len(data.len(), MessageFormat::Text, true);
                WebSocket::build_message(header, &data.to_string().into_bytes())
            }
            WebSocketMessage::Binary(data)=>{
                let header = MessageHeader::from_len(data.len(), MessageFormat::Text, true);
                WebSocket::build_message(header, &data)
            }
            _=>panic!()
        };
        unsafe {android_jni::to_java_websocket_send_message(self.request_id, frame);}

        Ok(())
    }
    
    pub fn open(request: HttpRequest, rx_sender:Sender<WebSocketMessage>)->OsWebSocket{
        let request_id = LiveId::unique();

        let sender_ref = Arc::new(Box::new(rx_sender));
        let pointer = Arc::as_ptr(&sender_ref);

        unsafe {android_jni::to_java_websocket_open(request_id, request, pointer);}

        OsWebSocket{
            sender_ref,
            request_id
        }
    }
}