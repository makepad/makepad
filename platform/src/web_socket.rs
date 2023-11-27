use crate::{
    os::OsWebSocket,
    cx_api::*,
    Cx,
    studio::AppToStudio,
    event::HttpMethod,
    event::HttpRequest,
    makepad_micro_serde::*
};

use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{channel, Sender, Receiver, TryRecvError,RecvError}
    }
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
pub (crate) static WEB_SOCKET_ID: AtomicU64 = AtomicU64::new(0);

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
impl Cx{
        
    pub(crate) fn start_studio_websocket(&mut self) {
        let studio_http: Option<&'static str> = std::option_env!("MAKEPAD_STUDIO_HTTP");
        if studio_http.is_none() {
            return
        }
        let url = format!("http://{}/$studio_web_socket", studio_http.unwrap());
        let request = HttpRequest::new(url, HttpMethod::GET);
        self.studio_web_socket = Some(WebSocket::open(request));
    }
    
    pub fn send_studio_message(msg:AppToStudio){
        let sender = WEB_SOCKET_THREAD_SENDER.lock().unwrap();
        if let Some(sender) = &*sender{
            let _= sender.send(WebSocketThreadMsg::SendMessage{
                socket_id: 0,
                message: WebSocketMessage::Binary(msg.serialize_bin()),
            });
        }
        else{
            panic!("Web socket thread not running")
        }
    }
}

impl WebSocket{    
    pub(crate) fn run_websocket_thread(cx:&mut Cx){
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
