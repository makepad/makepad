#[allow(unused_imports)]
use crate::{
    os::OsWebSocket,
    cx_api::*,
    Cx,
    studio::{AppToStudio,AppToStudioVec},
    event::{HttpMethod,HttpRequest},
    makepad_micro_serde::*
};
#[allow(unused_imports)]
use std::{
    time::{Instant, Duration},
    collections::HashMap,
    cell::RefCell,
    sync::{
        Mutex,
        atomic::{AtomicU64, AtomicBool, Ordering},
        mpsc::{channel, Sender, Receiver, RecvTimeoutError, TryRecvError,RecvError}
    }
};
     
#[derive(Debug)]
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

#[derive(Debug)]
pub enum WebSocketMessage{
    Error(String),
    Binary(Vec<u8>),
    String(String),
    Opened,
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
    
    #[cfg(target_arch = "wasm32")]
    fn run_websocket_thread(&mut self){
        let (rx_sender, rx_receiver) = channel();
        let mut thread_sender = WEB_SOCKET_THREAD_SENDER.lock().unwrap();
        *thread_sender = Some(rx_sender);
        let sockets = Mutex::new(RefCell::new(HashMap::new()));
        self.spawn_timer_thread(1, move ||{ 
            let mut app_to_studio = AppToStudioVec(Vec::new());
            while let Ok(msg) = rx_receiver.try_recv(){
                match msg{
                    WebSocketThreadMsg::Open{socket_id, request, rx_sender}=>{
                        let socket = OsWebSocket::open(socket_id, request, rx_sender);
                        sockets.lock().unwrap().borrow_mut().insert(socket_id, socket);
                    }
                    WebSocketThreadMsg::SendMessage{socket_id, message}=>{
                        if let Some(socket) = sockets.lock().unwrap().borrow_mut().get_mut(&socket_id){
                            socket.send_message(message).unwrap();
                        }
                    }
                    WebSocketThreadMsg::AppToStudio{message}=>{
                        app_to_studio.0.push(message);

                    }
                    WebSocketThreadMsg::Close{socket_id}=>{
                        sockets.lock().unwrap().borrow_mut().remove(&socket_id);
                    }
                }
            }
            if app_to_studio.0.len()>0{
                if let Some(socket) = sockets.lock().unwrap().borrow_mut().get_mut(&0){
                    socket.send_message(WebSocketMessage::Binary(app_to_studio.serialize_bin())).unwrap()
                }
            }
        });
    }
        
    #[cfg(not(target_arch = "wasm32"))]
    fn run_websocket_thread(&mut self){
        // lets create a thread
        let (rx_sender, rx_receiver) = channel();
        let mut thread_sender = WEB_SOCKET_THREAD_SENDER.lock().unwrap();
        *thread_sender = Some(rx_sender);
        self.spawn_thread(move ||{
            // this is the websocket thread.
            let mut sockets = HashMap::new();
            let mut app_to_studio = AppToStudioVec(Vec::new());
            let mut first_message = None;
            let collect_time = Duration::from_millis(16);
            let mut cycle_time = Duration::MAX;
            loop{
                // the idea is that this loop collects AppToStudio messages for a minimum of collect_time 
                // and then batches it. this solves flooding underlying platform websocket overhead (esp on web)
                match rx_receiver.recv_timeout(cycle_time){
                    Ok(msg)=>match msg{
                        WebSocketThreadMsg::Open{socket_id, request, rx_sender}=>{
                            let socket = OsWebSocket::open(socket_id, request, rx_sender);
                            sockets.insert(socket_id, socket);
                        }
                        WebSocketThreadMsg::SendMessage{socket_id, message}=>{
                            if let Some(socket) = sockets.get_mut(&socket_id){
                                socket.send_message(message).unwrap();
                            }
                        }
                        WebSocketThreadMsg::AppToStudio{message}=>{
                            if first_message.is_none(){
                                first_message = Some(Instant::now())
                            }
                            app_to_studio.0.push(message);
                            cycle_time = collect_time; // we should now block with a max of collect time since we received the first message
                        }
                        WebSocketThreadMsg::Close{socket_id}=>{
                            sockets.remove(&socket_id);
                        }
                    },
                    Err(RecvTimeoutError::Timeout)=>{ 
                    }
                    Err(RecvTimeoutError::Disconnected)=>{
                        return
                    }
                }
                if let Some(first_time) = first_message{
                    if Instant::now().duration_since(first_time) >= collect_time{
                        // lets send it
                        if let Some(socket) = sockets.get_mut(&0){
                            socket.send_message(WebSocketMessage::Binary(app_to_studio.serialize_bin())).unwrap();
                        }
                        app_to_studio.0.clear();
                        first_message = None;
                        cycle_time = Duration::MAX;
                    }
                }
            }
        });
    }
    
    fn start_studio_websocket(&mut self, studio_http: &str) {
        if studio_http.len() == 0{
            return
        }
        self.studio_http = studio_http.into();
        
        #[cfg(all(not(target_os="tvos"), not(target_os="ios")))]{
            // lets open a websocket
            HAS_STUDIO_WEB_SOCKET.store(true, Ordering::SeqCst);
            let request = HttpRequest::new(studio_http.to_string(), HttpMethod::GET);
            self.studio_web_socket = Some(WebSocket::open(request));
        }
        
    }
    
    #[cfg(any(target_os="tvos", target_os="ios"))]
    pub fn start_studio_websocket_delayed(&mut self) {
        HAS_STUDIO_WEB_SOCKET.store(true, Ordering::SeqCst);
        let request = HttpRequest::new(self.studio_http.clone(), HttpMethod::GET);
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
