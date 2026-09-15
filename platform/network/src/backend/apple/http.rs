use {
    crate::types::{HttpError, HttpProgress, HttpRequest, HttpResponse, NetworkResponse},
    makepad_apple_sys::*,
    makepad_apple_sys::objc_block_invoke,
    makepad_live_id::LiveId,
    std::{collections::BTreeMap, ptr, ptr::NonNull, sync::mpsc::Sender, sync::Once},
};

const URL_SESSION_DATA_DELEGATE_CLASS_NAME: &str = "MakepadNSURLSessionDataDelegate";
const URL_SESSION_BUFFER_DELEGATE_CLASS_NAME: &str = "MakepadNSURLSessionBufferDelegate";
const URL_SESSION_DELEGATE_CLASS_NAME: &str = "MakepadNSURLSessionDelegate";
const PROGRESS_EVERY: usize = 256 * 1024;

struct UrlSessionDataDelegateContext {
    sender: Sender<NetworkResponse>,
    request_id: LiveId,
    metadata_id: LiveId,
    received: u64,
    max_body_bytes: u64,
    is_head: bool,
    failed: bool,
}

fn url_session_data_delegate_class() -> *const Class {
    static INIT: Once = Once::new();
    static mut CLASS: *const Class = ptr::null();
    INIT.call_once(|| unsafe {
        CLASS = define_url_session_data_delegate();
    });
    unsafe { CLASS }
}

fn url_session_buffer_delegate_class() -> *const Class {
    static INIT: Once = Once::new();
    static mut CLASS: *const Class = ptr::null();
    INIT.call_once(|| unsafe {
        CLASS = define_url_session_buffer_delegate();
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
        this: &Object,
        _: Sel,
        _session: ObjcId,
        data_task: ObjcId,
        response: ObjcId,
        completion: ObjcId,
    ) {
        unsafe {
            let context_box: u64 = *this.get_ivar("context_box");
            let mut ctx: Box<UrlSessionDataDelegateContext> =
                Box::from_raw(context_box as *mut _);
            ctx.received = 0;
            let expected: i64 = msg_send![response, expectedContentLength];
            if !ctx.is_head && expected > 0 && expected as u64 > ctx.max_body_bytes {
                ctx.failed = true;
                let () = msg_send![data_task, cancel];
                let _ = Box::into_raw(ctx);
                objc_block_invoke!(completion, invoke((0): u64));
                return;
            }
            let _ = Box::into_raw(ctx);
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
            let mut context_box: Box<UrlSessionDataDelegateContext> =
                Box::from_raw(context_box as *mut _);

            let bytes: *const u8 = msg_send![data, bytes];
            let length: usize = msg_send![data, length];
            if context_box.failed
                || context_box.received.saturating_add(length as u64)
                    > context_box.max_body_bytes
            {
                context_box.failed = true;
                let () = msg_send![_data_task, cancel];
                let _ = Box::into_raw(context_box);
                return;
            }
            context_box.received += length as u64;
            let data_bytes: &[u8] = if bytes.is_null() || length == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(bytes, length)
            };

            if !data_bytes.is_empty() {
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
            }
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

            if context_box.failed {
                let message = NetworkResponse::HttpError {
                    request_id: context_box.request_id,
                    error: HttpError {
                        metadata_id: context_box.metadata_id,
                        message: crate::HTTP_BODY_LIMIT_ERROR.to_string(),
                    },
                };
                let _ = context_box.sender.send(message);
            } else if error != nil {
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

    extern "C" fn will_redirect(
        _this: &Object,
        _: Sel,
        _session: ObjcId,
        _task: ObjcId,
        _response: ObjcId,
        _request: ObjcId,
        completion: ObjcId,
    ) {
        unsafe {
            // Passing nil refuses the redirect and surfaces the original 3xx.
            objc_block_invoke!(completion, invoke((nil): ObjcId));
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
    if let Some(protocol) = Protocol::get("NSURLSessionDataDelegate") {
        decl.add_protocol(protocol);
    }

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
        decl.add_method(
            sel!(URLSession:task:willPerformHTTPRedirection:newRequest:completionHandler:),
            will_redirect
                as extern "C" fn(
                    &Object,
                    Sel,
                    ObjcId,
                    ObjcId,
                    ObjcId,
                    ObjcId,
                    ObjcId,
                ),
        );
    }

    decl.add_ivar::<u64>("context_box");
    decl.register()
}

struct UrlSessionBufferContext {
    sender: Sender<NetworkResponse>,
    request_id: LiveId,
    metadata_id: LiveId,
    body: Vec<u8>,
    expected: u64,
    last_emit: usize,
    status_code: u16,
    headers: BTreeMap<String, Vec<String>>,
    max_body_bytes: u64,
    is_head: bool,
    failed: bool,
}

fn emit_http_progress(ctx: &UrlSessionBufferContext) {
    let _ = ctx.sender.send(NetworkResponse::HttpProgress {
        request_id: ctx.request_id,
        progress: HttpProgress {
            loaded: ctx.body.len() as u64,
            total: ctx.expected,
        },
    });
}

pub fn define_url_session_buffer_delegate() -> *const Class {
    extern "C" fn did_receive_response(
        this: &Object,
        _: Sel,
        _session: ObjcId,
        _data_task: ObjcId,
        response: ObjcId,
        completion: ObjcId,
    ) {
        unsafe {
            let context_box: u64 = *this.get_ivar("context_box");
            let mut ctx: Box<UrlSessionBufferContext> = Box::from_raw(context_box as *mut _);
            let expected: i64 = msg_send![response, expectedContentLength];
            // New response (including after a 302) replaces any redirect body.
            ctx.body.clear();
            ctx.last_emit = 0;
            ctx.headers.clear();
            if expected > 0 {
                ctx.expected = expected as u64;
                if !ctx.is_head && ctx.expected > ctx.max_body_bytes {
                    ctx.failed = true;
                    let () = msg_send![_data_task, cancel];
                    let _ = Box::into_raw(ctx);
                    objc_block_invoke!(completion, invoke((0): u64));
                    return;
                }
                if !ctx.is_head {
                    if ctx.max_body_bytes == u64::MAX {
                        ctx.body.reserve(expected as usize);
                    } else {
                        ctx.body.reserve_exact(expected as usize);
                    }
                }
            } else {
                ctx.expected = 0;
            }
            let is_http: bool = msg_send![response, isKindOfClass: class!(NSHTTPURLResponse)];
            if is_http {
                ctx.status_code = msg_send![response, statusCode];
                let headers: ObjcId = msg_send![response, allHeaderFields];
                let key_enumerator: ObjcId = msg_send![headers, keyEnumerator];
                let mut key: ObjcId = msg_send![key_enumerator, nextObject];
                while key != ptr::null_mut() {
                    let value: ObjcId = msg_send![headers, objectForKey: key];
                    ctx.headers
                        .entry(nsstring_to_string(key))
                        .or_default()
                        .push(nsstring_to_string(value));
                    key = msg_send![key_enumerator, nextObject];
                }
            }
            emit_http_progress(&ctx);
            let _ = Box::into_raw(ctx);
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
            let mut ctx: Box<UrlSessionBufferContext> = Box::from_raw(context_box as *mut _);
            let bytes: *const u8 = msg_send![data, bytes];
            let length: usize = msg_send![data, length];
            if !ctx.failed
                && ctx.body.len().saturating_add(length) as u64 > ctx.max_body_bytes
            {
                ctx.failed = true;
                let () = msg_send![_data_task, cancel];
            } else if !ctx.failed && !bytes.is_null() && length > 0 {
                if ctx.max_body_bytes != u64::MAX {
                    ctx.body.reserve_exact(length);
                }
                ctx.body.extend_from_slice(std::slice::from_raw_parts(bytes, length));
            }
            if ctx.body.len().saturating_sub(ctx.last_emit) >= PROGRESS_EVERY {
                ctx.last_emit = ctx.body.len();
                emit_http_progress(&ctx);
            }
            let _ = Box::into_raw(ctx);
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
            let ctx: Box<UrlSessionBufferContext> = Box::from_raw(context_box as *mut _);
            if ctx.failed {
                let _ = ctx.sender.send(NetworkResponse::HttpError {
                    request_id: ctx.request_id,
                    error: HttpError {
                        metadata_id: ctx.metadata_id,
                        message: crate::HTTP_BODY_LIMIT_ERROR.to_string(),
                    },
                });
                return;
            }
            if error != nil {
                let error_str: String = nsstring_to_string(msg_send![error, localizedDescription]);
                let _ = ctx.sender.send(NetworkResponse::HttpError {
                    request_id: ctx.request_id,
                    error: HttpError {
                        metadata_id: ctx.metadata_id,
                        message: error_str,
                    },
                });
                return;
            }
            emit_http_progress(&ctx);
            let mut response = HttpResponse::new(
                ctx.metadata_id,
                ctx.status_code,
                Default::default(),
                Some(ctx.body),
            );
            for (key, values) in ctx.headers {
                for value in values {
                    response.set_header(key.clone(), value);
                }
            }
            let _ = ctx.sender.send(NetworkResponse::HttpResponse {
                request_id: ctx.request_id,
                response,
            });
        }
    }

    extern "C" fn will_redirect(
        _this: &Object,
        _: Sel,
        _session: ObjcId,
        _task: ObjcId,
        _response: ObjcId,
        _request: ObjcId,
        completion: ObjcId,
    ) {
        unsafe {
            objc_block_invoke!(completion, invoke((nil): ObjcId));
        }
    }

    if let Some(existing) = Class::get(URL_SESSION_BUFFER_DELEGATE_CLASS_NAME) {
        return existing as *const Class;
    }

    let superclass = class!(NSObject);
    let Some(mut decl) = ClassDecl::new(URL_SESSION_BUFFER_DELEGATE_CLASS_NAME, superclass) else {
        if let Some(existing) = Class::get(URL_SESSION_BUFFER_DELEGATE_CLASS_NAME) {
            return existing as *const Class;
        }
        return superclass as *const Class;
    };
    if let Some(protocol) = Protocol::get("NSURLSessionDataDelegate") {
        decl.add_protocol(protocol);
    }

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
        decl.add_method(
            sel!(URLSession:task:willPerformHTTPRedirection:newRequest:completionHandler:),
            will_redirect
                as extern "C" fn(
                    &Object,
                    Sel,
                    ObjcId,
                    ObjcId,
                    ObjcId,
                    ObjcId,
                    ObjcId,
                ),
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
    #[allow(dead_code)]
    session: Option<RcObjcId>,
    #[allow(dead_code)]
    session_delegate: Option<RcObjcId>,
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
            self.requests
                .retain(|request| request.request_id != request_id);
        }
    }

    pub fn make_http_request(
        &mut self,
        request_id: LiveId,
        request: HttpRequest,
        networking_sender: Sender<NetworkResponse>,
    ) {
        unsafe {
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
                let () = msg_send![config, setTimeoutIntervalForRequest: 120.0];
                // Large Range / zip downloads (TDM, Archive.org) need more
                // than two minutes or NSURLSession kills the transfer and we
                // retry, which is worse for the remote host.
                let () = msg_send![config, setTimeoutIntervalForResource: 1800.0];
                msg_send![class!(NSURLSession), sessionWithConfiguration: config delegate: nil delegateQueue:nil]
            };

            if is_streaming {
                let context_box = Box::into_raw(Box::new(UrlSessionDataDelegateContext {
                    request_id,
                    metadata_id,
                    sender: networking_sender,
                    received: 0,
                    max_body_bytes: request.max_response_body_bytes,
                    is_head: matches!(request.method, crate::types::HttpMethod::HEAD),
                    failed: false,
                })) as u64;
                let data_delegate_instance: ObjcId =
                    msg_send![url_session_data_delegate_class(), new];
                (*data_delegate_instance).set_ivar("context_box", context_box);

                let data_task: ObjcId = msg_send![session, dataTaskWithRequest: ns_request];
                let () = msg_send![data_task, setDelegate: data_delegate_instance];
                let () = msg_send![data_task, resume];
                self.requests.push(HttpReq {
                    request_id,
                    data_task: RcObjcId::from_unowned(NonNull::new(data_task).unwrap()),
                    session: None,
                    session_delegate: None,
                });
            } else {
                let context_box = Box::into_raw(Box::new(UrlSessionBufferContext {
                    request_id,
                    metadata_id,
                    sender: networking_sender,
                    body: Vec::new(),
                    expected: 0,
                    last_emit: 0,
                    status_code: 0,
                    headers: BTreeMap::new(),
                    max_body_bytes: request.max_response_body_bytes,
                    is_head: matches!(request.method, crate::types::HttpMethod::HEAD),
                    failed: false,
                })) as u64;
                let buffer_delegate: ObjcId =
                    msg_send![url_session_buffer_delegate_class(), new];
                (*buffer_delegate).set_ivar("context_box", context_box);
                let data_task: ObjcId = msg_send![session, dataTaskWithRequest: ns_request];
                let () = msg_send![data_task, setDelegate: buffer_delegate];
                let () = msg_send![data_task, resume];
                self.requests.push(HttpReq {
                    request_id,
                    data_task: RcObjcId::from_unowned(NonNull::new(data_task).unwrap()),
                    session: Some(RcObjcId::from_unowned(NonNull::new(session).unwrap())),
                    session_delegate: Some(RcObjcId::from_unowned(
                        NonNull::new(buffer_delegate).unwrap(),
                    )),
                });
            }
        }
    }
}
