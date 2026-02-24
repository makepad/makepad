use crate::makepad_live_id::LiveId;
use crate::makepad_network::{
    EventSink, HttpError, HttpProgress, HttpRequest, HttpResponse, NetworkBackend, NetworkError,
    NetworkResponse, WsMessage, WsSend,
};
use crate::makepad_wasm_bridge::WasmDataU8;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};

unsafe extern "C" {
    fn js_network_http_request(
        request_id_lo: u32,
        request_id_hi: u32,
        metadata_id_lo: u32,
        metadata_id_hi: u32,
        url_ptr: u32,
        url_len: u32,
        method_ptr: u32,
        method_len: u32,
        headers_ptr: u32,
        headers_len: u32,
        body_ptr: u32,
        body_len: u32,
    );
    fn js_network_http_cancel(request_id_lo: u32, request_id_hi: u32);

    fn js_network_ws_open(
        socket_id_lo: u32,
        socket_id_hi: u32,
        url_ptr: u32,
        url_len: u32,
        headers_ptr: u32,
        headers_len: u32,
    );
    fn js_network_ws_send_binary(
        socket_id_lo: u32,
        socket_id_hi: u32,
        data_ptr: u32,
        data_len: u32,
    );
    fn js_network_ws_send_text(
        socket_id_lo: u32,
        socket_id_hi: u32,
        data_ptr: u32,
        data_len: u32,
    );
    fn js_network_ws_close(socket_id_lo: u32, socket_id_hi: u32);
}

struct PendingHttp {
    public_request_id: LiveId,
    sink: EventSink,
}

#[derive(Default)]
struct HttpState {
    by_internal: HashMap<LiveId, PendingHttp>,
    by_public: HashMap<LiveId, LiveId>,
}

struct PendingWs {
    public_socket_id: LiveId,
    sink: EventSink,
}

#[derive(Default)]
struct WsState {
    by_internal: HashMap<LiveId, PendingWs>,
    by_public: HashMap<LiveId, LiveId>,
}

struct WasmNetworkShimBackend {
    next_internal_id: AtomicU64,
    http: Mutex<HttpState>,
    ws: Mutex<WsState>,
}

impl WasmNetworkShimBackend {
    fn new() -> Self {
        Self {
            next_internal_id: AtomicU64::new(1),
            http: Mutex::new(HttpState::default()),
            ws: Mutex::new(WsState::default()),
        }
    }

    fn next_internal_id(&self) -> LiveId {
        let raw = self.next_internal_id.fetch_add(1, Ordering::Relaxed) | (1u64 << 62);
        LiveId(raw)
    }

    fn emit_http_response(
        &self,
        internal_request_id: LiveId,
        metadata_id: LiveId,
        status_code: u16,
        headers: String,
        body: Vec<u8>,
    ) {
        let pending = {
            let mut state = match self.http.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let Some(pending) = state.by_internal.remove(&internal_request_id) else {
                return;
            };
            state.by_public.remove(&pending.public_request_id);
            pending
        };
        let _ = pending.sink.emit(NetworkResponse::HttpResponse {
            request_id: pending.public_request_id,
            response: HttpResponse::from_header_string(
                metadata_id,
                status_code,
                headers,
                Some(body),
            ),
        });
    }

    fn emit_http_error(
        &self,
        internal_request_id: LiveId,
        metadata_id: LiveId,
        message: String,
    ) {
        let pending = {
            let mut state = match self.http.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let Some(pending) = state.by_internal.remove(&internal_request_id) else {
                return;
            };
            state.by_public.remove(&pending.public_request_id);
            pending
        };
        let _ = pending.sink.emit(NetworkResponse::HttpError {
            request_id: pending.public_request_id,
            error: HttpError {
                message,
                metadata_id,
            },
        });
    }

    fn emit_http_progress(&self, internal_request_id: LiveId, loaded: u64, total: u64) {
        let pending = {
            let state = match self.http.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let Some(pending) = state.by_internal.get(&internal_request_id) else {
                return;
            };
            (pending.public_request_id, pending.sink.clone())
        };
        let _ = pending.1.emit(NetworkResponse::HttpProgress {
            request_id: pending.0,
            progress: HttpProgress { loaded, total },
        });
    }

    fn emit_ws_opened(&self, internal_socket_id: LiveId) {
        let pending = {
            let state = match self.ws.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let Some(pending) = state.by_internal.get(&internal_socket_id) else {
                return;
            };
            (pending.public_socket_id, pending.sink.clone())
        };
        let _ = pending.1.emit(NetworkResponse::WsOpened {
            socket_id: pending.0,
        });
    }

    fn emit_ws_closed(&self, internal_socket_id: LiveId) {
        let pending = {
            let mut state = match self.ws.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let Some(pending) = state.by_internal.remove(&internal_socket_id) else {
                return;
            };
            state.by_public.remove(&pending.public_socket_id);
            pending
        };
        let _ = pending.sink.emit(NetworkResponse::WsClosed {
            socket_id: pending.public_socket_id,
        });
    }

    fn emit_ws_error(&self, internal_socket_id: LiveId, message: String) {
        let pending = {
            let mut state = match self.ws.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let Some(pending) = state.by_internal.remove(&internal_socket_id) else {
                return;
            };
            state.by_public.remove(&pending.public_socket_id);
            pending
        };
        let _ = pending.sink.emit(NetworkResponse::WsError {
            socket_id: pending.public_socket_id,
            message,
        });
    }

    fn emit_ws_text(&self, internal_socket_id: LiveId, text: String) {
        let pending = {
            let state = match self.ws.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let Some(pending) = state.by_internal.get(&internal_socket_id) else {
                return;
            };
            (pending.public_socket_id, pending.sink.clone())
        };
        let _ = pending.1.emit(NetworkResponse::WsMessage {
            socket_id: pending.0,
            message: WsMessage::Text(text),
        });
    }

    fn emit_ws_binary(&self, internal_socket_id: LiveId, data: Vec<u8>) {
        let pending = {
            let state = match self.ws.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let Some(pending) = state.by_internal.get(&internal_socket_id) else {
                return;
            };
            (pending.public_socket_id, pending.sink.clone())
        };
        let _ = pending.1.emit(NetworkResponse::WsMessage {
            socket_id: pending.0,
            message: WsMessage::Binary(data),
        });
    }
}

impl NetworkBackend for WasmNetworkShimBackend {
    fn http_start(
        &self,
        request_id: LiveId,
        request: HttpRequest,
        sink: EventSink,
    ) -> Result<(), NetworkError> {
        let internal_request_id = self.next_internal_id();
        {
            let mut state = self
                .http
                .lock()
                .map_err(|_| NetworkError::backend("wasm shim http lock poisoned"))?;
            state.by_public.insert(request_id, internal_request_id);
            state.by_internal.insert(
                internal_request_id,
                PendingHttp {
                    public_request_id: request_id,
                    sink,
                },
            );
        }

        let metadata_id = request.metadata_id;
        let method = request.method;
        let headers_string = request.get_headers_string();
        let url = request.url;
        let body = request.body.unwrap_or_default();
        let method_string = method.as_str().to_string();

        unsafe {
            js_network_http_request(
                internal_request_id.lo(),
                internal_request_id.hi(),
                metadata_id.lo(),
                metadata_id.hi(),
                url.as_ptr() as u32,
                url.len() as u32,
                method_string.as_ptr() as u32,
                method_string.len() as u32,
                headers_string.as_ptr() as u32,
                headers_string.len() as u32,
                body.as_ptr() as u32,
                body.len() as u32,
            );
        }
        Ok(())
    }

    fn http_cancel(&self, request_id: LiveId) -> Result<(), NetworkError> {
        let internal_id = {
            let mut state = self
                .http
                .lock()
                .map_err(|_| NetworkError::backend("wasm shim http lock poisoned"))?;
            let Some(internal_id) = state.by_public.remove(&request_id) else {
                return Ok(());
            };
            state.by_internal.remove(&internal_id);
            internal_id
        };
        unsafe {
            js_network_http_cancel(internal_id.lo(), internal_id.hi());
        }
        Ok(())
    }

    fn ws_open(
        &self,
        socket_id: LiveId,
        request: HttpRequest,
        sink: EventSink,
    ) -> Result<(), NetworkError> {
        let internal_socket_id = self.next_internal_id();
        let url = request.url;
        let headers_string = request.get_headers_string();

        {
            let mut state = self
                .ws
                .lock()
                .map_err(|_| NetworkError::backend("wasm shim websocket lock poisoned"))?;
            state.by_public.insert(socket_id, internal_socket_id);
            state.by_internal.insert(
                internal_socket_id,
                PendingWs {
                    public_socket_id: socket_id,
                    sink,
                },
            );
        }

        unsafe {
            js_network_ws_open(
                internal_socket_id.lo(),
                internal_socket_id.hi(),
                url.as_ptr() as u32,
                url.len() as u32,
                headers_string.as_ptr() as u32,
                headers_string.len() as u32,
            );
        }
        Ok(())
    }

    fn ws_send(&self, socket_id: LiveId, message: WsSend) -> Result<(), NetworkError> {
        let internal_id = {
            let state = self
                .ws
                .lock()
                .map_err(|_| NetworkError::backend("wasm shim websocket lock poisoned"))?;
            *state.by_public.get(&socket_id).ok_or_else(|| {
                NetworkError::backend(format!("wasm shim websocket {socket_id} not open"))
            })?
        };
        match message {
            WsSend::Binary(data) => unsafe {
                js_network_ws_send_binary(
                    internal_id.lo(),
                    internal_id.hi(),
                    data.as_ptr() as u32,
                    data.len() as u32,
                );
            },
            WsSend::Text(data) => unsafe {
                js_network_ws_send_text(
                    internal_id.lo(),
                    internal_id.hi(),
                    data.as_ptr() as u32,
                    data.len() as u32,
                );
            },
        }
        Ok(())
    }

    fn ws_close(&self, socket_id: LiveId) -> Result<(), NetworkError> {
        let internal_id = {
            let mut state = self
                .ws
                .lock()
                .map_err(|_| NetworkError::backend("wasm shim websocket lock poisoned"))?;
            let Some(internal_id) = state.by_public.remove(&socket_id) else {
                return Ok(());
            };
            state.by_internal.remove(&internal_id);
            internal_id
        };
        unsafe {
            js_network_ws_close(internal_id.lo(), internal_id.hi());
        }
        Ok(())
    }
}

static INIT: Once = Once::new();
static SHIM_BACKEND: OnceLock<Arc<WasmNetworkShimBackend>> = OnceLock::new();

fn shim_backend() -> Option<&'static Arc<WasmNetworkShimBackend>> {
    SHIM_BACKEND.get()
}

pub(crate) fn install_network_backend_shim() {
    INIT.call_once(|| {
        let backend = Arc::new(WasmNetworkShimBackend::new());
        let _ = SHIM_BACKEND.set(backend.clone());
        crate::makepad_network::register_wasm_backend_shim(backend);
    });
}

#[export_name = "wasm_network_http_response"]
pub unsafe extern "C" fn wasm_network_http_response(
    request_id_lo: u32,
    request_id_hi: u32,
    metadata_id_lo: u32,
    metadata_id_hi: u32,
    status_code: u32,
    headers_ptr: u32,
    headers_len: u32,
    body_ptr: u32,
    body_len: u32,
) {
    let Some(backend) = shim_backend() else {
        return;
    };
    let headers = WasmDataU8::take_ownership(headers_ptr, headers_len, headers_len).into_utf8();
    let body = WasmDataU8::take_ownership(body_ptr, body_len, body_len).into_vec_u8();
    backend.emit_http_response(
        LiveId::from_lo_hi(request_id_lo, request_id_hi),
        LiveId::from_lo_hi(metadata_id_lo, metadata_id_hi),
        status_code as u16,
        headers,
        body,
    );
}

#[export_name = "wasm_network_http_error"]
pub unsafe extern "C" fn wasm_network_http_error(
    request_id_lo: u32,
    request_id_hi: u32,
    metadata_id_lo: u32,
    metadata_id_hi: u32,
    message_ptr: u32,
    message_len: u32,
) {
    let Some(backend) = shim_backend() else {
        return;
    };
    let message =
        WasmDataU8::take_ownership(message_ptr, message_len, message_len).into_utf8();
    backend.emit_http_error(
        LiveId::from_lo_hi(request_id_lo, request_id_hi),
        LiveId::from_lo_hi(metadata_id_lo, metadata_id_hi),
        message,
    );
}

#[export_name = "wasm_network_http_progress"]
pub unsafe extern "C" fn wasm_network_http_progress(
    request_id_lo: u32,
    request_id_hi: u32,
    loaded: u32,
    total: u32,
) {
    let Some(backend) = shim_backend() else {
        return;
    };
    backend.emit_http_progress(
        LiveId::from_lo_hi(request_id_lo, request_id_hi),
        loaded as u64,
        total as u64,
    );
}

#[export_name = "wasm_network_ws_opened"]
pub unsafe extern "C" fn wasm_network_ws_opened(socket_id_lo: u32, socket_id_hi: u32) {
    let Some(backend) = shim_backend() else {
        return;
    };
    backend.emit_ws_opened(LiveId::from_lo_hi(socket_id_lo, socket_id_hi));
}

#[export_name = "wasm_network_ws_closed"]
pub unsafe extern "C" fn wasm_network_ws_closed(socket_id_lo: u32, socket_id_hi: u32) {
    let Some(backend) = shim_backend() else {
        return;
    };
    backend.emit_ws_closed(LiveId::from_lo_hi(socket_id_lo, socket_id_hi));
}

#[export_name = "wasm_network_ws_error"]
pub unsafe extern "C" fn wasm_network_ws_error(
    socket_id_lo: u32,
    socket_id_hi: u32,
    message_ptr: u32,
    message_len: u32,
) {
    let Some(backend) = shim_backend() else {
        return;
    };
    let message =
        WasmDataU8::take_ownership(message_ptr, message_len, message_len).into_utf8();
    backend.emit_ws_error(LiveId::from_lo_hi(socket_id_lo, socket_id_hi), message);
}

#[export_name = "wasm_network_ws_text"]
pub unsafe extern "C" fn wasm_network_ws_text(
    socket_id_lo: u32,
    socket_id_hi: u32,
    data_ptr: u32,
    data_len: u32,
) {
    let Some(backend) = shim_backend() else {
        return;
    };
    let text = WasmDataU8::take_ownership(data_ptr, data_len, data_len).into_utf8();
    backend.emit_ws_text(LiveId::from_lo_hi(socket_id_lo, socket_id_hi), text);
}

#[export_name = "wasm_network_ws_binary"]
pub unsafe extern "C" fn wasm_network_ws_binary(
    socket_id_lo: u32,
    socket_id_hi: u32,
    data_ptr: u32,
    data_len: u32,
) {
    let Some(backend) = shim_backend() else {
        return;
    };
    let data = WasmDataU8::take_ownership(data_ptr, data_len, data_len).into_vec_u8();
    backend.emit_ws_binary(LiveId::from_lo_hi(socket_id_lo, socket_id_hi), data);
}
