use crate::{
    os::OsWebSocket,
    cx_api::*,
    Cx,
    studio::{AppToStudio},
    event::{HttpMethod,HttpRequest},
    makepad_micro_serde::*
};

use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, AtomicBool, Ordering},
        mpsc::{channel, Sender, Receiver, RecvError, TryRecvError}
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
    },
    AppToStudio{
        message: AppToStudio
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
pub (crate) static HAS_STUDIO_WEB_SOCKET: AtomicBool = AtomicBool::new(false);

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
    pub(crate) fn has_studio_web_socket()->bool{ 
       HAS_STUDIO_WEB_SOCKET.load(Ordering::SeqCst)
    }
    
    fn run_websocket_thread(&mut self){
        // lets create a thread
        let (rx_sender, rx_receiver) = channel();
        let mut thread_sender = WEB_SOCKET_THREAD_SENDER.lock().unwrap();
        *thread_sender = Some(rx_sender);
        self.spawn_thread(move ||{
            // this is the websocket thread.
            let mut sockets = HashMap::new();
            loop{
                match rx_receiver.recv(){ 
                    Ok(msg)=>match msg{
                        WebSocketThreadMsg::Open{socket_id, request, rx_sender}=>{
                            let socket = OsWebSocket::open(request, rx_sender);
                            sockets.insert(socket_id, socket);
                        }
                        WebSocketThreadMsg::SendMessage{socket_id, message}=>{
                            if let Some(socket) = sockets.get_mut(&socket_id){
                                socket.send_message(message).unwrap();
                            }
                        }
                        WebSocketThreadMsg::AppToStudio{message}=>{
                            if let Some(socket) = sockets.get_mut(&0){
                                socket.send_message(WebSocketMessage::Binary(message.serialize_bin())).unwrap();
                            }
                        }
                        WebSocketThreadMsg::Close{socket_id}=>{
                            sockets.remove(&socket_id);
                        }
                    },
                    Err(_)=>{
                        return
                    }
                }
            }
        });
    }
    
    fn start_studio_websocket(&mut self, studio_http: &str) {
        if studio_http.len() == 0{
            return
        }
        // lets open a websocket
        HAS_STUDIO_WEB_SOCKET.store(true, Ordering::SeqCst);
        let request = HttpRequest::new(studio_http.to_string(), HttpMethod::GET);
        self.studio_web_socket = Some(WebSocket::open(request));
    }
    
    pub fn init_websockets(&mut self, studio_http: &str) {
        self.run_websocket_thread();
        self.start_studio_websocket(studio_http);
    }
    
    pub fn send_studio_message(msg:AppToStudio){
        if !Cx::has_studio_web_socket(){
            return
        }
        let sender = WEB_SOCKET_THREAD_SENDER.lock().unwrap();
        if let Some(sender) = &*sender{
            let _= sender.send(WebSocketThreadMsg::AppToStudio{message:msg});
        }
        else{
            println!("Web socket thread not running (yet) for {:?}", msg);
        }
    }
}

impl WebSocket{    
    
    
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
