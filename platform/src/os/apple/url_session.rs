
use {
    std::{
        ptr,
        sync::mpsc::{Sender},
        sync::Arc,
    },
    crate::{
        makepad_live_id::*,
        makepad_objc_sys::{objc_block, objc_block_invoke},
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
            NetworkResponseItem,
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
    
    return decl.register();
}

// this allows for locally signed SSL certificates to pass
// TODO make this configurable
pub fn define_url_session_delegate() -> *const Class {
    extern fn did_receive_challenge(_this: &Object, _: Sel, _session: ObjcId, challenge: ObjcId, completion: ObjcId) {
        unsafe{
            let pspace: ObjcId = msg_send![challenge, protectionSpace];
            let trust: ObjcId = msg_send![pspace, serverTrust];
            if trust == nil{
                objc_block_invoke!(completion, invoke(
                    (0): usize,
                    (nil): ObjcId
                ));
            }
            else{
                let credential: ObjcId = msg_send![class!(NSURLCredential), credentialForTrust:trust];
                objc_block_invoke!(completion, invoke(
                    (0): usize,
                    (credential): ObjcId
                ));
            }
        }
    }
        
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("NSURLSessionDelegate", superclass).unwrap();
        
    // Add callback methods
    unsafe {
        decl.add_method(sel!(URLSession: didReceiveChallenge: completionHandler:), did_receive_challenge as extern fn(&Object, Sel, ObjcId,ObjcId, ObjcId));
    }
        
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

pub struct OsWebSocket{
    data_task: Arc<ObjcId>,
    rx_sender: Sender<WebSocketMessage>
}

impl OsWebSocket{

    pub fn send_message(&mut self, message:WebSocketMessage)->Result<(),()>{
        unsafe{
            let rx_sender = self.rx_sender.clone();
            let handler = objc_block!(move | error: ObjcId | {
                if error != ptr::null_mut() {
                    let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                    rx_sender.send(WebSocketMessage::Error(error_str)).unwrap();
                }
            });
                       
            let msg: ObjcId = match &message{
                WebSocketMessage::String(data)=>{
                    let nsstring = str_to_nsstring(data);
                    let msg:ObjcId = msg_send![class!(NSURLSessionWebSocketMessage), alloc];
                    let () = msg_send![msg, initWithString: nsstring];
                    msg
                }
                WebSocketMessage::Binary(data)=>{
                    let nsdata: ObjcId = msg_send![class!(NSData), dataWithBytes: data.as_ptr() length: data.len()];
                    let msg: ObjcId = msg_send![class!(NSURLSessionWebSocketMessage), alloc];
                    let () = msg_send![msg, initWithData: nsdata];
                    msg
                }
                _=>panic!()
            };
                        
            let () = msg_send![*Arc::as_ptr(&self.data_task), sendMessage: msg completionHandler: &handler];
            Ok(())
        } 
    }
            
    pub fn open(request: HttpRequest, rx_sender:Sender<WebSocketMessage>)->OsWebSocket{
        unsafe {
            
            //self.lazy_init_ns_url_session();
            let ns_request = make_ns_request(&request);
            
            let session: ObjcId = if request.ignore_ssl_cert{
                let config: ObjcId = msg_send![class!(NSURLSessionConfiguration), defaultSessionConfiguration];
                let deleg: ObjcId = msg_send![get_apple_class_global().url_session_delegate, new];
                msg_send![class!(NSURLSession), sessionWithConfiguration: config delegate: deleg delegateQueue:nil]
            }     
            else{
                msg_send![class!(NSURLSession), sharedSession]
            };
            
            //let session  = self.ns_url_session.unwrap();
            let data_task: ObjcId = msg_send![session, webSocketTaskWithRequest: ns_request];
            let web_socket_delegate_instance: ObjcId = msg_send![get_apple_class_global().web_socket_delegate, new];
                                
            unsafe fn set_message_receive_handler(data_task2: Arc<ObjcId>,  rx_sender: Sender<WebSocketMessage>) {
                let data_task = data_task2.clone();
                let handler = objc_block!(move | message: ObjcId, error: ObjcId | {
                    if error != ptr::null_mut() {
                        
                        let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                        rx_sender.send(WebSocketMessage::Error(error_str)).unwrap();
                        return;
                    }
                    let ty: usize = msg_send![message, type];
                    if ty == 0 { // binary
                        let data: ObjcId = msg_send![message, data];
                        let bytes: *const u8 = msg_send![data, bytes];
                        let length: usize = msg_send![data, length];
                        let data_bytes: &[u8] = std::slice::from_raw_parts(bytes, length);
                        let message = WebSocketMessage::Binary(data_bytes.to_vec());
                        rx_sender.send(message).unwrap();
                    }
                    else { // string
                        let string: ObjcId = msg_send![message, string];
                        let message = WebSocketMessage::String(nsstring_to_string(string));
                        rx_sender.send(message).unwrap();
                    }
                    set_message_receive_handler(data_task.clone(), rx_sender.clone())
                });
                let () = msg_send![*Arc::as_ptr(&data_task2), receiveMessageWithCompletionHandler: &handler];
            }
            let () = msg_send![data_task, setDelegate: web_socket_delegate_instance];
            let data_task = Arc::new(data_task);
            set_message_receive_handler(data_task.clone(), rx_sender.clone());
            let () = msg_send![*Arc::as_ptr(&data_task), resume];
            OsWebSocket{
                rx_sender,
                data_task
            }
        }
    }
        
}


pub fn make_http_request(request_id: LiveId, request: HttpRequest, networking_sender: Sender<NetworkResponseItem>) {
    unsafe {
        let ignore_ssl_cert = request.ignore_ssl_cert;
        let ns_request = make_ns_request(&request);
                
        // Build the NSURLSessionDataTask instance
        let response_handler = objc_block!(move | data: ObjcId, response: ObjcId, error: ObjcId | {
            if error != ptr::null_mut() {
                let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                let message = NetworkResponseItem {
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
                        
            let message = NetworkResponseItem {
                request_id,
                response: NetworkResponse::HttpResponse(response)
            };
            networking_sender.send(message).unwrap();
        });
        
        let session: ObjcId = if ignore_ssl_cert{
            let config: ObjcId = msg_send![class!(NSURLSessionConfiguration), defaultSessionConfiguration];
            let deleg: ObjcId = msg_send![get_apple_class_global().url_session_delegate, new];
            msg_send![class!(NSURLSession), sessionWithConfiguration: config delegate: deleg delegateQueue:nil]
        }     
        else{
            msg_send![class!(NSURLSession), sharedSession]
        };
                
        let data_task: ObjcId = msg_send![session, dataTaskWithRequest: ns_request completionHandler: &response_handler];
                
        // Run the request task
        let () = msg_send![data_task, resume];
    }
}