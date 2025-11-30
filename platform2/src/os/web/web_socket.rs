
use crate::event::HttpRequest;
use crate::web_socket::{WebSocketMessage};
use std::sync::mpsc::{Sender};
use std::sync::Mutex;
use std::cell::RefCell;
use makepad_wasm_bridge::WasmDataU8;
use crate::thread::SignalToUI;

pub struct OsWebSocket{
    id: u64
}

extern "C" {
    pub fn js_open_web_socket(id:u32, url_ptr: u32, url_len: u32);
    pub fn js_web_socket_send_string(id:u32, str_ptr: u32, url_len: u32);
    pub fn js_web_socket_send_binary(id:u32, bin_ptr: u32, bin_len: u32);
}

// alright we need a global set of websockets
static WEBSOCKET_LIST:Mutex<RefCell<Vec<(u32,Sender<WebSocketMessage>)>>> = Mutex::new(RefCell::new(Vec::new()));

#[export_name = "wasm_web_socket_closed"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_web_socket_closed(id: u32) {
    if let Ok(list) = WEBSOCKET_LIST.lock(){
        let mut list = list.borrow_mut();
        if let Some(index) = list.iter().position(|v| v.0 == id){
            let item = list.remove(index);
            let _ = item.1.send(WebSocketMessage::Closed);
            SignalToUI::set_ui_signal();
        }
    }
}

#[export_name = "wasm_web_socket_opened"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_web_socket_opened(id: u32) {
    if let Ok(list) = WEBSOCKET_LIST.lock(){
        if let Some(item) = list.borrow_mut().iter().find(|v| v.0 == id){
            let _ = item.1.send(WebSocketMessage::Opened);
            SignalToUI::set_ui_signal();
        }
    }
}

#[export_name = "wasm_web_socket_error"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_web_socket_error(id: u32, err_ptr:u32, err_len:u32) {
    let err = WasmDataU8::take_ownership(err_ptr, err_len, err_len).into_utf8();
    if let Ok(list) = WEBSOCKET_LIST.lock(){
        if let Some(item) = list.borrow_mut().iter().find(|v| v.0 == id){
            let _ = item.1.send(WebSocketMessage::Error(err));
            SignalToUI::set_ui_signal();
        }
    }
}

#[export_name = "wasm_web_socket_string"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_web_socket_string(id: u32, str_ptr:u32, str_len:u32) {
    let str = WasmDataU8::take_ownership(str_ptr, str_len, str_len).into_utf8();
    if let Ok(list) = WEBSOCKET_LIST.lock(){
        if let Some(item) = list.borrow_mut().iter().find(|v| v.0 == id){
            let _ = item.1.send(WebSocketMessage::String(str));
            SignalToUI::set_ui_signal();
        }
    }
}

#[export_name = "wasm_web_socket_binary"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_web_socket_binary(id: u32, u8_ptr:u32, u8_len:u32) {
    let bin = WasmDataU8::take_ownership(u8_ptr, u8_len, u8_len).into_vec_u8();
    if let Ok(list) = WEBSOCKET_LIST.lock(){
        if let Some(item) = list.borrow_mut().iter().find(|v| v.0 == id){
            let _ = item.1.send(WebSocketMessage::Binary(bin));
            SignalToUI::set_ui_signal();
        }
    }
}

impl OsWebSocket{
    pub fn send_message(&mut self, message:WebSocketMessage)->Result<(),()>{
        // alright lets send a message
        match message{
            WebSocketMessage::Binary(bin)=>{
                unsafe{js_web_socket_send_binary(self.id as u32, bin.as_ptr() as u32, bin.len() as u32)};                
            }
            WebSocketMessage::String(str)=>{
                let bytes = str.as_bytes();
                unsafe{js_web_socket_send_string(self.id as u32, bytes.as_ptr() as u32, bytes.len() as u32)};
            }
            _=>()
        }
        //todo!();
        Ok(())
    }
    
    pub fn close(&mut self){
        
    }
                
    pub fn open(id:u64, request: HttpRequest, rx_sender:Sender<WebSocketMessage>)->OsWebSocket{
        // alright lets call out directly from wasm to JS
        if let Ok(list) = WEBSOCKET_LIST.lock(){
            list.borrow_mut().push((id as u32, rx_sender));
        }
        let mut url = request.url;
        url = url.replace("https://","wss://");
        url = url.replace("http://","ws://");
        let bytes = url.as_bytes();
        
        unsafe{js_open_web_socket(id as u32, bytes.as_ptr() as u32, bytes.len() as u32)};
        OsWebSocket{id}
    }
}