//! Retained network service boundary for clients built by another compiler.
//!
//! The runtime and its Arc stay with the creator. Callbacks borrow views only
//! for the duration of a call; collections are reconstructed by their owner.

use crate::{
    HttpMethod, HttpRequest, HttpResponse, NetworkResponse, NetworkRuntime, WebSocketTransport,
    WsMessage,
};
use makepad_live_id::LiveId;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::Arc;

#[repr(C)]
pub struct NetworkHeaderView<'a> {
    pub name: &'a str,
    pub value: &'a str,
}

fn header_views(headers: &BTreeMap<String, Vec<String>>) -> Vec<NetworkHeaderView<'_>> {
    headers
        .iter()
        .flat_map(|(name, values)| {
            values
                .iter()
                .map(move |value| NetworkHeaderView { name, value })
        })
        .collect()
}

fn owned_headers(headers: &[NetworkHeaderView<'_>]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for header in headers {
        out.entry(header.name.to_owned())
            .or_default()
            .push(header.value.to_owned());
    }
    out
}

#[repr(C)]
pub struct NetworkRequestView<'a> {
    pub metadata_id: LiveId,
    pub url: &'a str,
    pub method: &'a str,
    pub headers: &'a [NetworkHeaderView<'a>],
    pub ignore_ssl_cert: bool,
    pub is_streaming: bool,
    pub max_response_body_bytes: u64,
    pub has_body: bool,
    pub body: &'a [u8],
    /// 0 = automatic, 1 = plain TCP, 2 = platform backend.
    pub websocket_transport: u8,
}

impl NetworkRequestView<'_> {
    fn to_owned(&self) -> HttpRequest {
        HttpRequest {
            metadata_id: self.metadata_id,
            url: self.url.to_owned(),
            method: match self.method {
                "GET" => HttpMethod::GET,
                "HEAD" => HttpMethod::HEAD,
                "POST" => HttpMethod::POST,
                "PUT" => HttpMethod::PUT,
                "DELETE" => HttpMethod::DELETE,
                "CONNECT" => HttpMethod::CONNECT,
                "OPTIONS" => HttpMethod::OPTIONS,
                "TRACE" => HttpMethod::TRACE,
                "PATCH" => HttpMethod::PATCH,
                _ => unreachable!("invalid network method"),
            },
            headers: owned_headers(self.headers),
            ignore_ssl_cert: self.ignore_ssl_cert,
            is_streaming: self.is_streaming,
            max_response_body_bytes: self.max_response_body_bytes,
            body: self.has_body.then(|| self.body.to_vec()),
            websocket_transport: match self.websocket_transport {
                0 => WebSocketTransport::Auto,
                1 => WebSocketTransport::PlainTcp,
                2 => WebSocketTransport::Platform,
                _ => unreachable!("invalid websocket transport"),
            },
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy)]
pub enum NetworkResponseKind {
    HttpResponse,
    HttpStreamChunk,
    HttpStreamComplete,
    HttpError,
    HttpProgress,
    WsOpened,
    WsBinary,
    WsText,
    WsClosed,
    WsError,
}

#[repr(C)]
pub struct NetworkResponseView<'a> {
    pub kind: NetworkResponseKind,
    pub id: LiveId,
    pub metadata_id: LiveId,
    pub status_code: u16,
    pub headers: &'a [NetworkHeaderView<'a>],
    pub has_body: bool,
    pub body: &'a [u8],
    pub message: &'a str,
    pub loaded: u64,
    pub total: u64,
}

impl NetworkResponseView<'_> {
    pub fn to_owned(&self) -> NetworkResponse {
        use NetworkResponseKind as K;
        let response = || HttpResponse {
            metadata_id: self.metadata_id,
            status_code: self.status_code,
            headers: owned_headers(self.headers),
            body: self.has_body.then(|| Arc::from(self.body)),
        };
        match self.kind {
            K::HttpResponse => NetworkResponse::HttpResponse {
                request_id: self.id,
                response: response(),
            },
            K::HttpStreamChunk => NetworkResponse::HttpStreamChunk {
                request_id: self.id,
                response: response(),
            },
            K::HttpStreamComplete => NetworkResponse::HttpStreamComplete {
                request_id: self.id,
                response: response(),
            },
            K::HttpError => NetworkResponse::HttpError {
                request_id: self.id,
                error: crate::HttpError {
                    metadata_id: self.metadata_id,
                    message: self.message.to_owned(),
                },
            },
            K::HttpProgress => NetworkResponse::HttpProgress {
                request_id: self.id,
                progress: crate::HttpProgress {
                    loaded: self.loaded,
                    total: self.total,
                },
            },
            K::WsOpened => NetworkResponse::WsOpened { socket_id: self.id },
            K::WsBinary => NetworkResponse::WsMessage {
                socket_id: self.id,
                message: WsMessage::Binary(self.body.to_vec()),
            },
            K::WsText => NetworkResponse::WsMessage {
                socket_id: self.id,
                message: WsMessage::Text(self.message.to_owned()),
            },
            K::WsClosed => NetworkResponse::WsClosed { socket_id: self.id },
            K::WsError => NetworkResponse::WsError {
                socket_id: self.id,
                message: self.message.to_owned(),
            },
        }
    }
}

impl NetworkResponse {
    /// Visit a borrowed representation without transferring Rust-owned values.
    pub fn with_view<R>(&self, f: impl FnOnce(&NetworkResponseView<'_>) -> R) -> R {
        use NetworkResponseKind as K;
        let mut headers = Vec::new();
        let mut view = NetworkResponseView {
            kind: K::WsOpened,
            id: LiveId::empty(),
            metadata_id: LiveId::empty(),
            status_code: 0,
            headers: &[],
            has_body: false,
            body: &[],
            message: "",
            loaded: 0,
            total: 0,
        };
        match self {
            Self::HttpResponse {
                request_id,
                response,
            }
            | Self::HttpStreamChunk {
                request_id,
                response,
            }
            | Self::HttpStreamComplete {
                request_id,
                response,
            } => {
                view.kind = match self {
                    Self::HttpResponse { .. } => K::HttpResponse,
                    Self::HttpStreamChunk { .. } => K::HttpStreamChunk,
                    _ => K::HttpStreamComplete,
                };
                view.id = *request_id;
                view.metadata_id = response.metadata_id;
                view.status_code = response.status_code;
                headers.extend(header_views(&response.headers));
                view.headers = &headers;
                view.has_body = response.body.is_some();
                view.body = response.body.as_deref().unwrap_or_default();
            }
            Self::HttpError { request_id, error } => {
                view.kind = K::HttpError;
                view.id = *request_id;
                view.metadata_id = error.metadata_id;
                view.message = &error.message;
            }
            Self::HttpProgress {
                request_id,
                progress,
            } => {
                view.kind = K::HttpProgress;
                view.id = *request_id;
                view.loaded = progress.loaded;
                view.total = progress.total;
            }
            Self::WsOpened { socket_id } => {
                view.kind = K::WsOpened;
                view.id = *socket_id;
            }
            Self::WsClosed { socket_id } => {
                view.kind = K::WsClosed;
                view.id = *socket_id;
            }
            Self::WsError { socket_id, message } => {
                view.kind = K::WsError;
                view.id = *socket_id;
                view.message = message;
            }
            Self::WsMessage { socket_id, message } => {
                view.id = *socket_id;
                match message {
                    WsMessage::Binary(data) => {
                        view.kind = K::WsBinary;
                        view.body = data;
                    }
                    WsMessage::Text(text) => {
                        view.kind = K::WsText;
                        view.message = text;
                    }
                }
            }
        }
        f(&view)
    }
}

// Views use Rust slice spelling for pointer/length pairs, as do the VM's
// existing C-shaped interfaces. They never contain owned Rust containers.
type ErrorSink = unsafe extern "C-unwind" fn(*mut c_void, *const u8, usize);
type ResponseSink = unsafe extern "C-unwind" fn(*mut c_void, &NetworkResponseView<'_>);

/// Owns one strong runtime reference, released by the runtime's compiler.
/// Its callbacks and context are private so safe code cannot forge ownership.
#[repr(C)]
pub struct NetworkRuntimeHandle {
    context: *const c_void,
    retain: unsafe extern "C-unwind" fn(*const c_void),
    release: unsafe extern "C-unwind" fn(*const c_void),
    request: unsafe extern "C-unwind" fn(
        *const c_void,
        bool,
        LiveId,
        &NetworkRequestView<'_>,
        *mut c_void,
        ErrorSink,
    ) -> bool,
    receive: unsafe extern "C-unwind" fn(*const c_void, *mut c_void, ResponseSink) -> bool,
}

impl From<Arc<NetworkRuntime>> for NetworkRuntimeHandle {
    fn from(runtime: Arc<NetworkRuntime>) -> Self {
        unsafe extern "C-unwind" fn retain(p: *const c_void) {
            Arc::increment_strong_count(p as *const NetworkRuntime);
        }
        unsafe extern "C-unwind" fn release(p: *const c_void) {
            Arc::decrement_strong_count(p as *const NetworkRuntime);
        }
        unsafe extern "C-unwind" fn request(
            p: *const c_void,
            ws: bool,
            id: LiveId,
            view: &NetworkRequestView<'_>,
            out: *mut c_void,
            error: ErrorSink,
        ) -> bool {
            let runtime = &*(p as *const NetworkRuntime);
            let result = if ws {
                runtime.ws_open(id, view.to_owned())
            } else {
                runtime.http_start(id, view.to_owned())
            };
            match result {
                Ok(()) => true,
                Err(err) => {
                    let message = err.to_string();
                    error(out, message.as_ptr(), message.len());
                    false
                }
            }
        }
        unsafe extern "C-unwind" fn receive(
            p: *const c_void,
            out: *mut c_void,
            sink: ResponseSink,
        ) -> bool {
            if let Some(response) = (&*(p as *const NetworkRuntime)).try_recv() {
                response.with_view(|view| sink(out, view));
                true
            } else {
                false
            }
        }
        Self {
            context: Arc::into_raw(runtime) as *const c_void,
            retain,
            release,
            request,
            receive,
        }
    }
}

impl Clone for NetworkRuntimeHandle {
    fn clone(&self) -> Self {
        unsafe {
            (self.retain)(self.context);
        }
        Self {
            context: self.context,
            retain: self.retain,
            release: self.release,
            request: self.request,
            receive: self.receive,
        }
    }
}

impl Drop for NetworkRuntimeHandle {
    fn drop(&mut self) {
        unsafe {
            (self.release)(self.context);
        }
    }
}

impl NetworkRuntimeHandle {
    fn start(&self, ws: bool, id: LiveId, request: HttpRequest) -> Result<(), String> {
        let headers = header_views(&request.headers);
        let view = NetworkRequestView {
            metadata_id: request.metadata_id,
            url: &request.url,
            method: request.method.as_str(),
            headers: &headers,
            ignore_ssl_cert: request.ignore_ssl_cert,
            is_streaming: request.is_streaming,
            max_response_body_bytes: request.max_response_body_bytes,
            has_body: request.body.is_some(),
            body: request.body.as_deref().unwrap_or_default(),
            websocket_transport: match request.websocket_transport {
                WebSocketTransport::Auto => 0,
                WebSocketTransport::PlainTcp => 1,
                WebSocketTransport::Platform => 2,
            },
        };
        unsafe extern "C-unwind" fn error(out: *mut c_void, message: *const u8, len: usize) {
            let bytes = std::slice::from_raw_parts(message, len);
            *(out as *mut String) = std::str::from_utf8(bytes)
                .expect("network error is UTF-8")
                .to_owned();
        }
        let mut message = String::new();
        if unsafe {
            (self.request)(
                self.context,
                ws,
                id,
                &view,
                &mut message as *mut String as *mut c_void,
                error,
            )
        } {
            Ok(())
        } else {
            Err(message)
        }
    }

    pub fn http_start(&self, id: LiveId, request: HttpRequest) -> Result<(), String> {
        self.start(false, id, request)
    }
    pub fn ws_open(&self, id: LiveId, request: HttpRequest) -> Result<(), String> {
        self.start(true, id, request)
    }

    pub fn try_recv(&self) -> Option<NetworkResponse> {
        unsafe extern "C-unwind" fn receive(out: *mut c_void, view: &NetworkResponseView<'_>) {
            *(out as *mut Option<NetworkResponse>) = Some(view.to_owned());
        }
        let mut response = None;
        unsafe {
            (self.receive)(
                self.context,
                &mut response as *mut Option<NetworkResponse> as *mut c_void,
                receive,
            );
        }
        response
    }
}

/// Scalar fields of server headers. Lines are supplied separately by their
/// owner, so no slice of Rust Strings crosses a compiler boundary.
#[repr(C)]
pub struct HttpServerHeadersView<'a> {
    pub addr_text: &'a str,
    pub verb: &'a str,
    pub path: &'a str,
    pub path_no_slash: &'a str,
    pub has_search: bool,
    pub search: &'a str,
    pub has_content_length: bool,
    pub content_length: u64,
    pub has_accept_encoding: bool,
    pub accept_encoding: &'a str,
    pub has_sec_websocket_key: bool,
    pub sec_websocket_key: &'a str,
}

impl crate::utils::HttpServerHeaders {
    #[inline]
    pub fn view(&self) -> HttpServerHeadersView<'_> {
        HttpServerHeadersView {
            addr_text: &self.addr_text,
            verb: &self.verb,
            path: &self.path,
            path_no_slash: &self.path_no_slash,
            has_search: self.search.is_some(),
            search: self.search.as_deref().unwrap_or_default(),
            has_content_length: self.content_length.is_some(),
            content_length: self.content_length.unwrap_or_default(),
            has_accept_encoding: self.accept_encoding.is_some(),
            accept_encoding: self.accept_encoding.as_deref().unwrap_or_default(),
            has_sec_websocket_key: self.sec_websocket_key.is_some(),
            sec_websocket_key: self.sec_websocket_key.as_deref().unwrap_or_default(),
        }
    }
}

/// Owns one global-allocator byte allocation. Fields are private and the value
/// is neither Copy nor Clone: moving transfers ownership, dropping frees it.
#[repr(C)]
pub struct NetworkOwnedBytes {
    ptr: *mut u8,
    len: usize,
    capacity: usize,
}
impl NetworkOwnedBytes {
    #[inline]
    fn new(bytes: Vec<u8>) -> Self {
        let mut bytes = std::mem::ManuallyDrop::new(bytes);
        Self {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
            capacity: bytes.capacity(),
        }
    }
    #[inline]
    fn into_vec(self) -> Vec<u8> {
        let this = std::mem::ManuallyDrop::new(self);
        // Both sides use the process global allocator (as for VM byte buffers).
        unsafe { Vec::from_raw_parts(this.ptr, this.len, this.capacity) }
    }
}
impl Drop for NetworkOwnedBytes {
    fn drop(&mut self) {
        unsafe { drop(Vec::from_raw_parts(self.ptr, self.len, self.capacity)) }
    }
}

/// Transfer of a byte-backed server response without passing String/Vec layouts.
/// The private header allocation always contains UTF-8, established by `new`.
/// If a receiver does not consume this value, its fields free both allocations.
#[repr(C)]
pub struct HttpServerResponseBuffers {
    header: NetworkOwnedBytes,
    body: NetworkOwnedBytes,
}
impl HttpServerResponseBuffers {
    #[inline]
    pub fn new(header: String, body: Vec<u8>) -> Self {
        Self {
            header: NetworkOwnedBytes::new(header.into_bytes()),
            body: NetworkOwnedBytes::new(body),
        }
    }
    #[inline]
    pub fn into_response(self) -> crate::http_server::HttpServerResponse {
        let Self { header, body } = self;
        // Only a String can initialize the private header; ownership moves once.
        let header = unsafe { String::from_utf8_unchecked(header.into_vec()) };
        crate::http_server::HttpServerResponse::new(header, body.into_vec())
    }
}
