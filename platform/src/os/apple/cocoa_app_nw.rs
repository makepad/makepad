
use {
    std::{
        ptr,
        os::raw::{c_void},
        sync::mpsc::Sender,
    },
    crate::{
        makepad_objc_sys::objc_block,
        makepad_objc_sys::runtime::{ObjcId},
        network::*,
        os::{
            cocoa_app::CocoaApp,
            apple::apple_sys::*,
            apple_util::{
                nsstring_to_string,
                str_to_nsstring,
            },
        },
        event::{
            AutoReconnect,
            HttpResponseEvent,
            HttpRequestErrorEvent,
        },
        NetworkingMessage,
    }
};

pub fn define_web_socket_delegate() -> *const Class {
    
    extern fn did_open_with_protocol(_this: &Object, _: Sel, _web_socket_task: ObjcId, _open_with_protocol: ObjcId) {
    }
    
    extern fn did_close_with_code(_this: &Object, _: Sel, _web_socket_task: ObjcId, _code: usize, _reason: ObjcId) {
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("NSURLSessionWebSocketDelegate", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(sel!(receivedTimer:), did_open_with_protocol as extern fn(&Object, Sel, ObjcId, ObjcId));
        decl.add_method(sel!(receivedLiveResize:), did_close_with_code as extern fn(&Object, Sel, ObjcId, usize, ObjcId));
    }
    // Store internal state as user data
    decl.add_ivar::<*mut c_void>("cocoa_app_ptr");
    
    return decl.register();
}

impl CocoaApp {
    
    unsafe fn make_ns_request(request:&HttpRequest)->ObjcId{
        // Prepare the NSMutableURLRequest instance
        let url: ObjcId =
        msg_send![class!(NSURL), URLWithString: str_to_nsstring(&request.url)];
        
        let mut ns_request: ObjcId = msg_send![class!(NSMutableURLRequest), alloc];
        ns_request = msg_send![ns_request, initWithURL: url];
        //let () = msg_send![ns_request, setHTTPMethod: str_to_nsstring(&request.method.to_string())];
        
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
        
    pub fn web_socket_open(&mut self, _socket_id: u64, request:HttpRequest, _auto_reconnect:AutoReconnect, _networking_sender: Sender<NetworkingMessage>) {
        
        unsafe {
            let _ns_request = Self::make_ns_request(&request);
            // Build the NSURLSessionDataTask instance
            /*
            let response_handler = objc_block!(move | data: ObjcId, response: ObjcId, error: ObjcId | {
                if error != ptr::null_mut() {
                    let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                    let message = NetworkingMessage::HttpRequestError(HttpRequestErrorEvent {
                        id: request.id.clone(),
                        error: error_str
                    });
                    networking_sender.send(message).unwrap();
                    return;
                }
                
                let bytes: *const u8 = msg_send![data, bytes];
                let length: usize = msg_send![data, length];
                let data_bytes: &[u8] = std::slice::from_raw_parts(bytes, length);
                let response_code: u16 = msg_send![response, statusCode];
                let headers: ObjcId = msg_send![response, allHeaderFields];
                
                let mut response = HttpResponse::new(
                    request.id.clone(),
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
                
                let message = NetworkingMessage::HttpResponse(HttpResponseEvent {response});
                networking_sender.send(message).unwrap();
            });
            */
            let _session: ObjcId = msg_send![class!(NSURLSession), sharedSession];
            /*
            let data_task: ObjcId = msg_send![session, webSocketTaskWithRequest: ns_request completionHandler: &response_handler];
            */
            // Run the request task
            //et () = msg_send![data_task, resume];
        }
    }
    
    pub fn make_http_request(&mut self, request: HttpRequest, networking_sender: Sender<NetworkingMessage>) {
        unsafe {
            let ns_request = Self::make_ns_request(&request);

            // Build the NSURLSessionDataTask instance
            let response_handler = objc_block!(move | data: ObjcId, response: ObjcId, error: ObjcId | {
                if error != ptr::null_mut() {
                    let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                    let message = NetworkingMessage::HttpRequestError(HttpRequestErrorEvent {
                        id: request.id.clone(),
                        error: error_str
                    });
                    networking_sender.send(message).unwrap();
                    return;
                }
                
                let bytes: *const u8 = msg_send![data, bytes];
                let length: usize = msg_send![data, length];
                let data_bytes: &[u8] = std::slice::from_raw_parts(bytes, length);
                let response_code: u16 = msg_send![response, statusCode];
                let headers: ObjcId = msg_send![response, allHeaderFields];
                
                let mut response = HttpResponse::new(
                    request.id.clone(),
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
                
                let message = NetworkingMessage::HttpResponse(HttpResponseEvent {response});
                networking_sender.send(message).unwrap();
            });
            
            let session: ObjcId = msg_send![class!(NSURLSession), sharedSession];
            let data_task: ObjcId = msg_send![session, dataTaskWithRequest: ns_request completionHandler: &response_handler];
            
            // Run the request task
            let () = msg_send![data_task, resume];
        }
    }
}
