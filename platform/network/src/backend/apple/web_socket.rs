use {
    crate::types::{HttpRequest, WebSocketMessage},
    makepad_apple_sys::objc_block,
    makepad_apple_sys::*,
    makepad_live_id::LiveId,
    std::{ptr, sync::mpsc::Sender, sync::Arc, sync::Once},
};

use super::http::{make_ns_request, url_session_delegate_class};

const WEB_SOCKET_DELEGATE_CLASS_NAME: &str = "MakepadNSURLSessionWebSocketDelegate";

fn web_socket_delegate_class() -> *const Class {
    static INIT: Once = Once::new();
    static mut CLASS: *const Class = ptr::null();
    INIT.call_once(|| unsafe {
        CLASS = define_web_socket_delegate();
    });
    unsafe { CLASS }
}

pub fn define_web_socket_delegate() -> *const Class {
    extern "C" fn did_open_with_protocol(
        _this: &Object,
        _: Sel,
        _web_socket_task: ObjcId,
        _open_with_protocol: ObjcId,
    ) {
    }

    extern "C" fn did_close_with_code(
        _this: &Object,
        _: Sel,
        _web_socket_task: ObjcId,
        _code: usize,
        _reason: ObjcId,
    ) {
    }

    if let Some(existing) = Class::get(WEB_SOCKET_DELEGATE_CLASS_NAME) {
        return existing as *const Class;
    }

    let superclass = class!(NSObject);
    let Some(mut decl) = ClassDecl::new(WEB_SOCKET_DELEGATE_CLASS_NAME, superclass) else {
        if let Some(existing) = Class::get(WEB_SOCKET_DELEGATE_CLASS_NAME) {
            return existing as *const Class;
        }
        return superclass as *const Class;
    };
    unsafe {
        decl.add_method(
            sel!(webSocketTask: didOpenWithProtocol:),
            did_open_with_protocol as extern "C" fn(&Object, Sel, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(webSocketTask: didCloseWithCode: reason:),
            did_close_with_code as extern "C" fn(&Object, Sel, ObjcId, usize, ObjcId),
        );
    }
    decl.register()
}

pub struct AppleWebSocket {
    data_task: Arc<ObjcId>,
    rx_sender: Sender<WebSocketMessage>,
}

unsafe impl Send for AppleWebSocket {}
unsafe impl Sync for AppleWebSocket {}

impl AppleWebSocket {
    pub fn send_message(&mut self, message: WebSocketMessage) -> Result<(), ()> {
        unsafe {
            let rx_sender = self.rx_sender.clone();
            let handler = objc_block!(move |error: ObjcId| {
                if error != ptr::null_mut() {
                    let error_str: String =
                        nsstring_to_string(msg_send![error, localizedDescription]);
                    let _ = rx_sender.send(WebSocketMessage::Error(error_str));
                }
            });

            let msg: ObjcId = match &message {
                WebSocketMessage::String(data) => {
                    let nsstring = str_to_nsstring(data);
                    let message: ObjcId = msg_send![class!(NSURLSessionWebSocketMessage), alloc];
                    let () = msg_send![message, initWithString: nsstring];
                    message
                }
                WebSocketMessage::Binary(data) => {
                    let nsdata: ObjcId =
                        msg_send![class!(NSData), dataWithBytes: data.as_ptr() length: data.len()];
                    let message: ObjcId = msg_send![class!(NSURLSessionWebSocketMessage), alloc];
                    let () = msg_send![message, initWithData: nsdata];
                    message
                }
                WebSocketMessage::Closed => {
                    let () = msg_send![*Arc::as_ptr(&self.data_task), cancel];
                    return Ok(());
                }
                WebSocketMessage::Opened | WebSocketMessage::Error(_) => return Ok(()),
            };

            let () = msg_send![*Arc::as_ptr(&self.data_task), sendMessage: msg completionHandler: &handler];
            Ok(())
        }
    }

    pub fn close(&self) {
        unsafe {
            let () = msg_send![*self.data_task, cancel];
        }
    }

    pub fn open(
        _socket_id: LiveId,
        request: HttpRequest,
        rx_sender: Sender<WebSocketMessage>,
    ) -> AppleWebSocket {
        unsafe {
            let ns_request = make_ns_request(&request);

            let session: ObjcId = if request.ignore_ssl_cert {
                let config: ObjcId = msg_send![
                    class!(NSURLSessionConfiguration),
                    defaultSessionConfiguration
                ];
                let () = msg_send![config, setURLCache: nil];
                let delegate: ObjcId = msg_send![url_session_delegate_class(), new];
                msg_send![class!(NSURLSession), sessionWithConfiguration: config delegate: delegate delegateQueue:nil]
            } else {
                msg_send![class!(NSURLSession), sharedSession]
            };

            let data_task: ObjcId = msg_send![session, webSocketTaskWithRequest: ns_request];
            let web_socket_delegate_instance: ObjcId = msg_send![web_socket_delegate_class(), new];
            let () = msg_send![data_task, setMaximumMessageSize:5*1024*1024];

            fn set_message_receive_handler(
                data_task_ref: Arc<ObjcId>,
                rx_sender: Sender<WebSocketMessage>,
            ) {
                let data_task = data_task_ref.clone();
                let handler = objc_block!(move |message: ObjcId, error: ObjcId| {
                    unsafe {
                        if error != ptr::null_mut() {
                            let error_str: String =
                                nsstring_to_string(msg_send![error, localizedDescription]);
                            let _ = rx_sender.send(WebSocketMessage::Error(error_str));
                            return;
                        }

                        let message_type: usize = msg_send![message, type];
                        if message_type == 0 {
                            let data: ObjcId = msg_send![message, data];
                            let bytes: *const u8 = msg_send![data, bytes];
                            let length: usize = msg_send![data, length];
                            let data_bytes: &[u8] = std::slice::from_raw_parts(bytes, length);
                            let _ = rx_sender.send(WebSocketMessage::Binary(data_bytes.to_vec()));
                        } else {
                            let text: ObjcId = msg_send![message, string];
                            let _ =
                                rx_sender.send(WebSocketMessage::String(nsstring_to_string(text)));
                        }
                    }

                    set_message_receive_handler(data_task.clone(), rx_sender.clone());
                });

                unsafe {
                    let () = msg_send![*Arc::as_ptr(&data_task_ref), receiveMessageWithCompletionHandler: &handler];
                }
            }

            let () = msg_send![data_task, setDelegate: web_socket_delegate_instance];
            let data_task = Arc::new(data_task);
            set_message_receive_handler(data_task.clone(), rx_sender.clone());
            let () = msg_send![*Arc::as_ptr(&data_task), resume];

            AppleWebSocket {
                rx_sender,
                data_task,
            }
        }
    }
}
