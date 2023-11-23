
use {
    std::{
        ptr,
        sync::mpsc::{self, Receiver,Sender, TryRecvError, RecvError},
        sync::Arc,
    },
    crate::{
        makepad_live_id::*,
        makepad_objc_sys::{objc_block},
        makepad_objc_sys::runtime::{ObjcId},
        os::{
            //cocoa_app::{MacosApp, get_macos_class_global},
            apple::apple_sys::*,
            apple::apple_classes::get_apple_class_global,
            apple_util::{
                nsstring_to_string,
                str_to_nsstring,
            },
        },
        web_socket::WebSocketMessage,
        event::{
            NetworkResponseEvent,
            NetworkResponse,
            HttpRequest,
            HttpResponse
        },
    }
};

pub fn define_web_socket_delegate() -> *const Class {
    
    extern fn did_open_with_protocol(_this: &Object, _: Sel, _web_socket_task: ObjcId, _open_with_protocol: ObjcId) {
        crate::log!("DID OPEN WITH PROTOCOL");
    }
    
    extern fn did_close_with_code(_this: &Object, _: Sel, _web_socket_task: ObjcId, _code: usize, _reason: ObjcId) {
        crate::log!("DID CLOSE WITH CODE");
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("NSURLSessionWebSocketDelegate", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(sel!(webSocketTask: didOpenWithProtocol:), did_open_with_protocol as extern fn(&Object, Sel, ObjcId, ObjcId));
        decl.add_method(sel!(webSocketTask: didCloseWithCode: reason:), did_close_with_code as extern fn(&Object, Sel, ObjcId, usize, ObjcId));
    }
    // Store internal state as user data
    decl.add_ivar::<*mut c_void>("macos_app_ptr");
    
    return decl.register();
}

unsafe fn make_ns_request(request: &HttpRequest) -> ObjcId {
    // Prepare the NSMutableURLRequest instance
    let url: ObjcId =
    msg_send![class!(NSURL), URLWithString: str_to_nsstring(&request.url)];
    
    let mut ns_request: ObjcId = msg_send![class!(NSMutableURLRequest), alloc];
    ns_request = msg_send![ns_request, initWithURL: url];
    let () = msg_send![ns_request, setHTTPMethod: str_to_nsstring(&request.method.to_string())];
    
    for (key, values) in request.headers.iter() {
        for value in values {
            let () = msg_send![ns_request, addValue: str_to_nsstring(value) forHTTPHeaderField: str_to_nsstring(key)];
        }
    }
    
    if let Some(body) = request.body.as_ref() {
        let nsdata: ObjcId = msg_send![class!(NSData), dataWithBytes: body.as_ptr() length: body.len()];
        let () = msg_send![ns_request, setHTTPBody: nsdata];
    }
    ns_request
}
/*
pub struct OsWebSocket{
    recv: Receiver<WebSocketMessage>,
    data_task: Arc<ObjcId>,
    sender: Sender<WebSocketMessage>
}

impl OsWebSocket{
    pub fn try_recv(&mut self)->Result<WebSocketMessage,TryRecvError>{
        self.recv.try_recv()
    }
    
    pub fn recv(&mut self)->Result<WebSocketMessage,RecvError>{
        self.recv.recv()
    }
    
    pub fn send_binary(&mut self, data:&[u8])->Result<(),()>{
        unsafe{
            let sender = self.sender.clone();
            let handler = objc_block!(move | error: ObjcId | {
                if error != ptr::null_mut() {
                    let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                    sender.send(WebSocketMessage::Error(error_str)).unwrap();
                }
            });
            let nsdata: ObjcId = msg_send![class!(NSData), dataWithBytes: data.as_ptr() length: data.len()];
            let msg: ObjcId = msg_send![class!(NSURLSessionWebSocketMessage), initWithData: nsdata];
            let () = msg_send![*Arc::as_ptr(&self.data_task), sendMessage: msg completionHandler:handler];
            Ok(())
        } 
    }
        
    pub fn send_string(&mut self, data:&str)->Result<(),()>{
        unsafe{
            let sender = self.sender.clone();
            let handler = objc_block!(move | error: ObjcId | {
                if error != ptr::null_mut() {
                    let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                    sender.send(WebSocketMessage::Error(error_str)).unwrap();
                }
            });
            let nsstring = str_to_nsstring(data);
            let msg: ObjcId = msg_send![class!(NSURLSessionWebSocketMessage), initWithString: nsstring];
            let () = msg_send![*Arc::as_ptr(&self.data_task), sendMessage: msg completionHandler:handler];
            Ok(())
        } 
    }
    
    pub fn open<F>(request: HttpRequest, rx_callback:F, tx_receiver:Receiver<WebSocketMessage>)->OsWebSocket where F:Fn(WebSocketMessage) + Send + 'static{
        // lets create a websocket
        // lets spawn a thread to run the websocket on
        // and on this thread we run the receiver loop
        
        // and connect up the receivers / senders
        
        unsafe {
            let (sender,recv) = mpsc::channel();
            //self.lazy_init_ns_url_session();
            let ns_request = make_ns_request(&request);
            let session: ObjcId = msg_send![class!(NSURLSession), sharedSession];
            //let session  = self.ns_url_session.unwrap();
            let data_task: ObjcId = msg_send![session, webSocketTaskWithRequest: ns_request];
            let web_socket_delegate_instance: ObjcId = msg_send![get_apple_class_global().web_socket_delegate, new];
                    
            unsafe fn set_message_receive_handler(data_task2: Arc<ObjcId>,  sender: Sender<WebSocketMessage>) {
                let data_task = data_task2.clone();
                let handler = objc_block!(move | message: ObjcId, error: ObjcId | {
                    if error != ptr::null_mut() {
                        let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                        sender.send(WebSocketMessage::Error(error_str)).unwrap();
                        return;
                    }
                    let ty: usize = msg_send![message, type];
                    if ty == 0 { // binary
                        let data: ObjcId = msg_send![message, data];
                        let bytes: *const u8 = msg_send![data, bytes];
                        let length: usize = msg_send![data, length];
                        let data_bytes: &[u8] = std::slice::from_raw_parts(bytes, length);
                        let message = WebSocketMessage::Binary(data_bytes.to_vec());
                        sender.send(message).unwrap();
                    }
                    else { // string
                        let string: ObjcId = msg_send![message, string];
                        let message = WebSocketMessage::String(nsstring_to_string(string));
                        sender.send(message).unwrap();
                    }
                    set_message_receive_handler(data_task.clone(), sender.clone())
                });
                let () = msg_send![*Arc::as_ptr(&data_task2), receiveMessageWithCompletionHandler: handler];
            }
            let () = msg_send![data_task, setDelegate: web_socket_delegate_instance];
            let data_task = Arc::new(data_task);
            set_message_receive_handler(data_task.clone(), sender.clone());
            let () = msg_send![*Arc::as_ptr(&data_task), resume];
            OsWebSocket{
                sender,
                recv,
                data_task
            }
        }
    }
    
}


pub fn make_http_request(request_id: LiveId, request: HttpRequest, networking_sender: Sender<NetworkResponseEvent>) {
    unsafe {
        let ns_request = make_ns_request(&request);
        
        // Build the NSURLSessionDataTask instance
        let response_handler = objc_block!(move | data: ObjcId, response: ObjcId, error: ObjcId | {
            if error != ptr::null_mut() {
                let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                let message = NetworkResponseEvent {
                    request_id,
                    response: NetworkResponse::HttpRequestError(error_str)
                };
                networking_sender.send(message).unwrap();
                return;
            }
            
            let bytes: *const u8 = msg_send![data, bytes];
            let length: usize = msg_send![data, length];
            let data_bytes: &[u8] = std::slice::from_raw_parts(bytes, length);
            let response_code: u16 = msg_send![response, statusCode];
            let headers: ObjcId = msg_send![response, allHeaderFields];
            let mut response = HttpResponse::new(
                request.metadata_id,
                response_code,
                "".to_string(),
                Some(data_bytes.to_vec()),
            );
            
            let key_enumerator: ObjcId = msg_send![headers, keyEnumerator];
            let mut key: ObjcId = msg_send![key_enumerator, nextObject];
            while key != ptr::null_mut() {
                let value: ObjcId = msg_send![headers, objectForKey: key];
                let key_str = nsstring_to_string(key);
                let value_str = nsstring_to_string(value);
                response.set_header(key_str, value_str);
                
                key = msg_send![key_enumerator, nextObject];
            }
            
            let message = NetworkResponseEvent {
                request_id,
                response: NetworkResponse::HttpResponse(response)
            };
            networking_sender.send(message).unwrap();
        });
        
        let session: ObjcId = msg_send![class!(NSURLSession), sharedSession];
        let data_task: ObjcId = msg_send![session, dataTaskWithRequest: ns_request completionHandler: &response_handler];
        
        // Run the request task
        let () = msg_send![data_task, resume];
    }
}
*/

pub struct OsWebSocket{
    recv: Receiver<WebSocketMessage>,
    data_task: Arc<ObjcId>,
    sender: Sender<WebSocketMessage>
}

impl OsWebSocket{
    pub fn try_recv(&mut self)->Result<WebSocketMessage,TryRecvError>{
        self.recv.try_recv()
    }
        
    pub fn recv(&mut self)->Result<WebSocketMessage,RecvError>{
        self.recv.recv()
    }
        
    pub fn send_binary(&mut self, data:&[u8])->Result<(),()>{
        unsafe{
            let sender = self.sender.clone();
            let handler = objc_block!(move | error: ObjcId | {
                if error != ptr::null_mut() {
                    let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                    sender.send(WebSocketMessage::Error(error_str)).unwrap();
                }
            });
            let nsdata: ObjcId = msg_send![class!(NSData), dataWithBytes: data.as_ptr() length: data.len()];
            let msg: ObjcId = msg_send![class!(NSURLSessionWebSocketMessage), initWithData: nsdata];
            let () = msg_send![*Arc::as_ptr(&self.data_task), sendMessage: msg completionHandler:handler];
            Ok(())
        } 
    }
            
    pub fn send_string(&mut self, data:&str)->Result<(),()>{
        unsafe{
            let sender = self.sender.clone();
            let handler = objc_block!(move | error: ObjcId | {
                if error != ptr::null_mut() {
                    let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                    sender.send(WebSocketMessage::Error(error_str)).unwrap();
                }
            });
            let nsstring = str_to_nsstring(data);
            let msg: ObjcId = msg_send![class!(NSURLSessionWebSocketMessage), initWithString: nsstring];
            let () = msg_send![*Arc::as_ptr(&self.data_task), sendMessage: msg completionHandler:handler];
            Ok(())
        } 
    }
        
    pub fn open(request: HttpRequest)->OsWebSocket{
        unsafe {
            let (sender,recv) = mpsc::channel();
            //self.lazy_init_ns_url_session();
            let ns_request = make_ns_request(&request);
            let session: ObjcId = msg_send![class!(NSURLSession), sharedSession];
            //let session  = self.ns_url_session.unwrap();
            let data_task: ObjcId = msg_send![session, webSocketTaskWithRequest: ns_request];
            let web_socket_delegate_instance: ObjcId = msg_send![get_apple_class_global().web_socket_delegate, new];
                                
            unsafe fn set_message_receive_handler(data_task2: Arc<ObjcId>,  sender: Sender<WebSocketMessage>) {
                let data_task = data_task2.clone();
                let handler = objc_block!(move | message: ObjcId, error: ObjcId | {
                    if error != ptr::null_mut() {
                        let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                        sender.send(WebSocketMessage::Error(error_str)).unwrap();
                        return;
                    }
                    let ty: usize = msg_send![message, type];
                    if ty == 0 { // binary
                        let data: ObjcId = msg_send![message, data];
                        let bytes: *const u8 = msg_send![data, bytes];
                        let length: usize = msg_send![data, length];
                        let data_bytes: &[u8] = std::slice::from_raw_parts(bytes, length);
                        let message = WebSocketMessage::Binary(data_bytes.to_vec());
                        sender.send(message).unwrap();
                    }
                    else { // string
                        let string: ObjcId = msg_send![message, string];
                        let message = WebSocketMessage::String(nsstring_to_string(string));
                        sender.send(message).unwrap();
                    }
                    set_message_receive_handler(data_task.clone(), sender.clone())
                });
                let () = msg_send![*Arc::as_ptr(&data_task2), receiveMessageWithCompletionHandler: handler];
            }
            let () = msg_send![data_task, setDelegate: web_socket_delegate_instance];
            let data_task = Arc::new(data_task);
            set_message_receive_handler(data_task.clone(), sender.clone());
            let () = msg_send![*Arc::as_ptr(&data_task), resume];
            OsWebSocket{
                sender,
                recv,
                data_task
            }
        }
    }
        
}


pub fn make_http_request(request_id: LiveId, request: HttpRequest, networking_sender: Sender<NetworkResponseEvent>) {
    unsafe {
        let ns_request = make_ns_request(&request);
                
        // Build the NSURLSessionDataTask instance
        let response_handler = objc_block!(move | data: ObjcId, response: ObjcId, error: ObjcId | {
            if error != ptr::null_mut() {
                let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                let message = NetworkResponseEvent {
                    request_id,
                    response: NetworkResponse::HttpRequestError(error_str)
                };
                networking_sender.send(message).unwrap();
                return;
            }
                        
            let bytes: *const u8 = msg_send![data, bytes];
            let length: usize = msg_send![data, length];
            let data_bytes: &[u8] = std::slice::from_raw_parts(bytes, length);
            let response_code: u16 = msg_send![response, statusCode];
            let headers: ObjcId = msg_send![response, allHeaderFields];
            let mut response = HttpResponse::new(
                request.metadata_id,
                response_code,
                "".to_string(),
                Some(data_bytes.to_vec()),
            );
                        
            let key_enumerator: ObjcId = msg_send![headers, keyEnumerator];
            let mut key: ObjcId = msg_send![key_enumerator, nextObject];
            while key != ptr::null_mut() {
                let value: ObjcId = msg_send![headers, objectForKey: key];
                let key_str = nsstring_to_string(key);
                let value_str = nsstring_to_string(value);
                response.set_header(key_str, value_str);
                                
                key = msg_send![key_enumerator, nextObject];
            }
                        
            let message = NetworkResponseEvent {
                request_id,
                response: NetworkResponse::HttpResponse(response)
            };
            networking_sender.send(message).unwrap();
        });
                
        let session: ObjcId = msg_send![class!(NSURLSession), sharedSession];
        let data_task: ObjcId = msg_send![session, dataTaskWithRequest: ns_request completionHandler: &response_handler];
                
        // Run the request task
        let () = msg_send![data_task, resume];
    }
}