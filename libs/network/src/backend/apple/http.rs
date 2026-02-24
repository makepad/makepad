use {
    crate::types::{HttpError, HttpRequest, HttpResponse, NetworkResponse},
    makepad_apple_sys::{objc_block, objc_block_invoke},
    makepad_apple_sys::*,
    makepad_live_id::LiveId,
    std::{ptr, ptr::NonNull, sync::mpsc::Sender, sync::Once},
};

const URL_SESSION_DATA_DELEGATE_CLASS_NAME: &str = "MakepadNSURLSessionDataDelegate";
const URL_SESSION_DELEGATE_CLASS_NAME: &str = "MakepadNSURLSessionDelegate";

struct UrlSessionDataDelegateContext {
    sender: Sender<NetworkResponse>,
    request_id: LiveId,
    metadata_id: LiveId,
}

fn url_session_data_delegate_class() -> *const Class {
    static INIT: Once = Once::new();
    static mut CLASS: *const Class = ptr::null();
    INIT.call_once(|| unsafe {
        CLASS = define_url_session_data_delegate();
    });
    unsafe { CLASS }
}

pub(crate) fn url_session_delegate_class() -> *const Class {
    static INIT: Once = Once::new();
    static mut CLASS: *const Class = ptr::null();
    INIT.call_once(|| unsafe {
        CLASS = define_url_session_delegate();
    });
    unsafe { CLASS }
}

pub fn define_url_session_data_delegate() -> *const Class {
    extern "C" fn did_receive_response(
        _this: &Object,
        _: Sel,
        _session: ObjcId,
        _data_task: ObjcId,
        _response: ObjcId,
        completion: ObjcId,
    ) {
        unsafe {
            objc_block_invoke!(completion, invoke((NSURLSessionResponseAllow): u64));
        }
    }

    extern "C" fn did_receive_data(
        this: &Object,
        _: Sel,
        _session: ObjcId,
        _data_task: ObjcId,
        data: ObjcId,
    ) {
        unsafe {
            let context_box: u64 = *this.get_ivar("context_box");
            let context_box: Box<UrlSessionDataDelegateContext> =
                Box::from_raw(context_box as *mut _);

            let bytes: *const u8 = msg_send![data, bytes];
            let length: usize = msg_send![data, length];
            let data_bytes: &[u8] = std::slice::from_raw_parts(bytes, length);

            let message = NetworkResponse::HttpStreamChunk {
                request_id: context_box.request_id,
                response: HttpResponse {
                    headers: Default::default(),
                    metadata_id: context_box.metadata_id,
                    status_code: 0,
                    body: Some(data_bytes.to_vec()),
                },
            };

            let _ = context_box.sender.send(message);
            let _ = Box::into_raw(context_box);
        }
    }

    extern "C" fn did_complete_with_error(
        this: &Object,
        _: Sel,
        _session: ObjcId,
        _task: ObjcId,
        error: ObjcId,
    ) {
        unsafe {
            let context_box: u64 = *this.get_ivar("context_box");
            let context_box: Box<UrlSessionDataDelegateContext> =
                Box::from_raw(context_box as *mut _);

            if error != nil {
                let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                let message = NetworkResponse::HttpError {
                    request_id: context_box.request_id,
                    error: HttpError {
                        metadata_id: context_box.metadata_id,
                        message: error_str,
                    },
                };
                let _ = context_box.sender.send(message);
            } else {
                let message = NetworkResponse::HttpStreamComplete {
                    request_id: context_box.request_id,
                    response: HttpResponse {
                        headers: Default::default(),
                        metadata_id: context_box.metadata_id,
                        status_code: 0,
                        body: None,
                    },
                };
                let _ = context_box.sender.send(message);
            }
        }
    }

    if let Some(existing) = Class::get(URL_SESSION_DATA_DELEGATE_CLASS_NAME) {
        return existing as *const Class;
    }

    let superclass = class!(NSObject);
    let Some(mut decl) = ClassDecl::new(URL_SESSION_DATA_DELEGATE_CLASS_NAME, superclass) else {
        if let Some(existing) = Class::get(URL_SESSION_DATA_DELEGATE_CLASS_NAME) {
            return existing as *const Class;
        }
        return superclass as *const Class;
    };

    unsafe {
        decl.add_method(
            sel!(URLSession:dataTask:didReceiveResponse:completionHandler:),
            did_receive_response as extern "C" fn(&Object, Sel, ObjcId, ObjcId, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(URLSession:dataTask:didReceiveData:),
            did_receive_data as extern "C" fn(&Object, Sel, ObjcId, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(URLSession:task:didCompleteWithError:),
            did_complete_with_error as extern "C" fn(&Object, Sel, ObjcId, ObjcId, ObjcId),
        );
    }

    decl.add_ivar::<u64>("context_box");
    decl.register()
}

// This allows locally signed SSL certificates to pass.
pub fn define_url_session_delegate() -> *const Class {
    extern "C" fn did_receive_challenge(
        _this: &Object,
        _: Sel,
        _session: ObjcId,
        challenge: ObjcId,
        completion: ObjcId,
    ) {
        unsafe {
            let pspace: ObjcId = msg_send![challenge, protectionSpace];
            let trust: ObjcId = msg_send![pspace, serverTrust];
            if trust == nil {
                objc_block_invoke!(completion, invoke((0): usize, (nil): ObjcId));
            } else {
                let credential: ObjcId =
                    msg_send![class!(NSURLCredential), credentialForTrust:trust];
                objc_block_invoke!(completion, invoke((0): usize, (credential): ObjcId));
            }
        }
    }

    if let Some(existing) = Class::get(URL_SESSION_DELEGATE_CLASS_NAME) {
        return existing as *const Class;
    }

    let superclass = class!(NSObject);
    let Some(mut decl) = ClassDecl::new(URL_SESSION_DELEGATE_CLASS_NAME, superclass) else {
        if let Some(existing) = Class::get(URL_SESSION_DELEGATE_CLASS_NAME) {
            return existing as *const Class;
        }
        return superclass as *const Class;
    };
    unsafe {
        decl.add_method(
            sel!(URLSession: didReceiveChallenge: completionHandler:),
            did_receive_challenge as extern "C" fn(&Object, Sel, ObjcId, ObjcId, ObjcId),
        );
    }
    decl.register()
}

pub(crate) unsafe fn make_ns_request(request: &HttpRequest) -> ObjcId {
    let url: ObjcId = msg_send![class!(NSURL), URLWithString: str_to_nsstring(&request.url)];
    let mut ns_request: ObjcId = msg_send![class!(NSMutableURLRequest), alloc];

    ns_request = msg_send![ns_request, initWithURL: url];
    let () = msg_send![ns_request, setHTTPMethod: str_to_nsstring(&request.method.as_str())];

    for (key, values) in request.headers.iter() {
        for value in values {
            let () = msg_send![ns_request, addValue: str_to_nsstring(value) forHTTPHeaderField: str_to_nsstring(key)];
        }
    }

    if let Some(body) = request.body.as_ref() {
        let nsdata: ObjcId =
            msg_send![class!(NSData), dataWithBytes: body.as_ptr() length: body.len()];
        let () = msg_send![ns_request, setHTTPBody: nsdata];
    }

    ns_request
}

struct HttpReq {
    request_id: LiveId,
    data_task: RcObjcId,
}

#[derive(Default)]
pub struct AppleHttpRequests {
    requests: Vec<HttpReq>,
}

impl AppleHttpRequests {
    pub fn cancel_http_request(&mut self, request_id: LiveId) {
        self.requests.retain(|request| {
            if request.request_id == request_id {
                unsafe {
                    let () = msg_send![request.data_task.as_id(), cancel];
                }
                false
            } else {
                true
            }
        });
    }

    pub fn handle_response(&mut self, response: &NetworkResponse) {
        let completed_id = match response {
            NetworkResponse::HttpError { request_id, .. }
            | NetworkResponse::HttpResponse { request_id, .. }
            | NetworkResponse::HttpStreamComplete { request_id, .. } => Some(*request_id),
            _ => None,
        };

        if let Some(request_id) = completed_id {
            self.requests.retain(|request| request.request_id != request_id);
        }
    }

    pub fn make_http_request(
        &mut self,
        request_id: LiveId,
        request: HttpRequest,
        networking_sender: Sender<NetworkResponse>,
    ) {
        unsafe {
            if request.url.contains("/v1/chat/completions") {
                if let Some(body) = request.body.as_ref() {
                    println!(
                        "[network debug] outgoing chat body: {}",
                        String::from_utf8_lossy(body)
                    );
                } else {
                    println!("[network debug] outgoing chat body: <none>");
                }
            }
            let ignore_ssl_cert = request.ignore_ssl_cert;
            let is_streaming = request.is_streaming;
            let metadata_id = request.metadata_id;
            let ns_request = make_ns_request(&request);

            let session: ObjcId = if ignore_ssl_cert {
                let config: ObjcId = msg_send![
                    class!(NSURLSessionConfiguration),
                    defaultSessionConfiguration
                ];
                let () = msg_send![config, setURLCache: nil];
                let delegate: ObjcId = msg_send![url_session_delegate_class(), new];
                msg_send![class!(NSURLSession), sessionWithConfiguration: config delegate: delegate delegateQueue:nil]
            } else {
                let config: ObjcId = msg_send![
                    class!(NSURLSessionConfiguration),
                    defaultSessionConfiguration
                ];
                let () = msg_send![config, setURLCache: nil];
                let () = msg_send![config, setTimeoutIntervalForRequest: 60.0];
                let () = msg_send![config, setTimeoutIntervalForResource: 120.0];
                msg_send![class!(NSURLSession), sessionWithConfiguration: config delegate: nil delegateQueue:nil]
            };

            if is_streaming {
                let context_box = Box::into_raw(Box::new(UrlSessionDataDelegateContext {
                    request_id,
                    metadata_id,
                    sender: networking_sender,
                })) as u64;
                let data_delegate_instance: ObjcId = msg_send![url_session_data_delegate_class(), new];
                (*data_delegate_instance).set_ivar("context_box", context_box);

                let data_task: ObjcId = msg_send![session, dataTaskWithRequest: ns_request];
                let () = msg_send![data_task, setDelegate: data_delegate_instance];
                let () = msg_send![data_task, resume];
                self.requests.push(HttpReq {
                    request_id,
                    data_task: RcObjcId::from_unowned(NonNull::new(data_task).unwrap()),
                });
            } else {
                let sender = networking_sender.clone();
                let response_handler = objc_block!(move |data: ObjcId, response: ObjcId, error: ObjcId| {
                    if error != ptr::null_mut() {
                        let error_str: String =
                            nsstring_to_string(msg_send![error, localizedDescription]);
                        let _ = sender.send(NetworkResponse::HttpError {
                            request_id,
                            error: HttpError {
                                metadata_id,
                                message: error_str,
                            },
                        });
                        return;
                    }

                    let bytes: *const u8 = msg_send![data, bytes];
                    let length: usize = msg_send![data, length];
                    let data_bytes: &[u8] = std::slice::from_raw_parts(bytes, length);
                    let status_code: u16 = msg_send![response, statusCode];
                    let headers: ObjcId = msg_send![response, allHeaderFields];

                    let mut http_response = HttpResponse::new(
                        metadata_id,
                        status_code,
                        Default::default(),
                        Some(data_bytes.to_vec()),
                    );

                    let key_enumerator: ObjcId = msg_send![headers, keyEnumerator];
                    let mut key: ObjcId = msg_send![key_enumerator, nextObject];
                    while key != ptr::null_mut() {
                        let value: ObjcId = msg_send![headers, objectForKey: key];
                        let key_str = nsstring_to_string(key);
                        let value_str = nsstring_to_string(value);
                        http_response.set_header(key_str, value_str);
                        key = msg_send![key_enumerator, nextObject];
                    }

                    let _ = sender.send(NetworkResponse::HttpResponse {
                        request_id,
                        response: http_response,
                    });
                });

                let data_task: ObjcId = msg_send![session, dataTaskWithRequest: ns_request completionHandler: &response_handler];
                let () = msg_send![data_task, resume];
                self.requests.push(HttpReq {
                    request_id,
                    data_task: RcObjcId::from_unowned(NonNull::new(data_task).unwrap()),
                });
            }
        }
    }
}
