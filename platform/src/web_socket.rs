use crate::os::OsWebSocket;
use crate::cx_api::*;
use crate::Cx;
use crate::event::HttpRequest;
use std::collections::HashMap;
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc::{channel, Sender, Receiver, TryRecvError,RecvError}
};

pub enum WebSocketThreadMsg{
    Open{
        socket_id: u64,
        request:HttpRequest,
        rx_sender: Sender<WebSocketMessage>
    },
    Close{
        socket_id: u64
    },
    SendMessage{
        socket_id: u64,
        message: WebSocketMessage
    }
}

pub struct WebSocket{
    socket_id: u64,
    pub rx_receiver: Receiver<WebSocketMessage>,
}

pub enum WebSocketMessage{
    Error(String),
    Binary(Vec<u8>),
    String(String),
    Closed
}

pub (crate) static WEB_SOCKET_THREAD_SENDER: Mutex<Option<Sender<WebSocketThreadMsg>>> = Mutex::new(None);
pub (crate) static WEB_SOCKET_ID: AtomicU64 = AtomicU64::new(1);

impl Drop for WebSocket{
    fn drop(&mut self){
        let sender = WEB_SOCKET_THREAD_SENDER.lock().unwrap();
        if let Some(sender) = &*sender{
            sender.send(WebSocketThreadMsg::Close{
                socket_id: self.socket_id,
            }).unwrap();
        }
    }
}

impl WebSocket{    
    pub fn run_websocket_thread(cx:&mut Cx){
        // lets create a thread
        let (rx_sender, rx_receiver) = channel();
        let mut thread_sender = WEB_SOCKET_THREAD_SENDER.lock().unwrap();
        *thread_sender = Some(rx_sender);
        cx.spawn_thread(move ||{
            // this is the websocket thread.
            let mut sockets = HashMap::new();
            while let Ok(msg) = rx_receiver.recv(){
                match msg{
                    WebSocketThreadMsg::Open{socket_id, request, rx_sender}=>{
                        let socket = OsWebSocket::open(request, rx_sender);
                        sockets.insert(socket_id, socket);
                    }
                    WebSocketThreadMsg::SendMessage{socket_id, message}=>{
                        // lets look up our OsWebSocket and send a message
                        if let Some(socket) = sockets.get_mut(&socket_id){
                            socket.send_message(message).unwrap();
                        }
                    }
                    WebSocketThreadMsg::Close{socket_id}=>{
                        sockets.remove(&socket_id);
                    }
                }
            }
        });
    }
    
    pub fn open(request:HttpRequest)->WebSocket {
        let (rx_sender, rx_receiver) = channel();
        let sender = WEB_SOCKET_THREAD_SENDER.lock().unwrap();
        let socket_id = WEB_SOCKET_ID.fetch_add(1, Ordering::SeqCst);
        if let Some(sender) = &*sender{
            sender.send(WebSocketThreadMsg::Open{
                socket_id,
                rx_sender,
                request
            }).unwrap();
            }
        else{
            panic!("Web socket thread not running")
        }
        WebSocket{
            socket_id,
            rx_receiver,
        }
    }
    
    pub fn send_binary(&mut self, data:Vec<u8>)->Result<(),()>{
        let sender = WEB_SOCKET_THREAD_SENDER.lock().unwrap();
        if let Some(sender) = &*sender{
            sender.send(WebSocketThreadMsg::SendMessage{
                socket_id: self.socket_id,
                message: WebSocketMessage::Binary(data),
            }).map_err(|_|())
        }
        else{
            panic!("Web socket thread not running")
        }
    }
    
    pub fn send_string(&mut self, data:String)->Result<(),()>{
        let sender = WEB_SOCKET_THREAD_SENDER.lock().unwrap();
        if let Some(sender) = &*sender{
            sender.send(WebSocketThreadMsg::SendMessage{
                socket_id: self.socket_id,
                message: WebSocketMessage::String(data),
            }).map_err(|_|())
        }
        else{
            panic!("Web socket thread not running")
        }
    }
    
    pub fn try_recv(&mut self)->Result<WebSocketMessage,TryRecvError>{
        self.rx_receiver.try_recv()
    }
    
    pub fn recv(&mut self)->Result<WebSocketMessage,RecvError>{
        self.rx_receiver.recv()
    }
}
