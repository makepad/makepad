use super::android_jni;
use crate::makepad_live_id::LiveId;
use crate::makepad_network::{
    AndroidSocketStream, AndroidSocketStreamFactory, EventSink, HttpError, HttpRequest,
    HttpResponse, NetworkBackend, NetworkError, NetworkResponse, ServerWebSocketMessage,
    ServerWebSocketMessageFormat, ServerWebSocketMessageHeader, WebSocketMessage, WebSocketParser,
    WsMessage, WsSend,
};
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::Duration;

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
    internal_socket_id: LiveId,
    sink: EventSink,
    socket: AndroidWebSocket,
}

struct AndroidWebSocket {
    _sender_ref: Arc<Box<(u64, Sender<WebSocketMessage>)>>,
    java_socket_id: LiveId,
}

fn io_other(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg.into())
}

struct AndroidSocketStreamFactoryImpl;

struct AndroidSocketStreamImpl {
    socket_id: LiveId,
    is_shutdown: bool,
}

impl Drop for AndroidSocketStreamImpl {
    fn drop(&mut self) {
        if !self.is_shutdown {
            unsafe {
                android_jni::to_java_socket_stream_close(self.socket_id);
            }
            self.is_shutdown = true;
        }
    }
}

impl AndroidSocketStreamFactory for AndroidSocketStreamFactoryImpl {
    fn connect(
        &self,
        host: &str,
        port: &str,
        use_tls: bool,
        ignore_ssl_cert: bool,
    ) -> io::Result<Box<dyn AndroidSocketStream>> {
        let port = port
            .parse::<u16>()
            .map_err(|_| io_other(format!("invalid port for android socket stream: {port}")))?;
        let socket_id = LiveId::unique();

        let opened = unsafe {
            android_jni::to_java_socket_stream_open(
                socket_id,
                host,
                port as i32,
                use_tls,
                ignore_ssl_cert,
            )
        };
        if !opened {
            return Err(io_other(
                "android platform socket stream open failed on Java side",
            ));
        }

        Ok(Box::new(AndroidSocketStreamImpl {
            socket_id,
            is_shutdown: false,
        }))
    }
}

fn timeout_to_ms(timeout: Option<Duration>) -> i32 {
    match timeout {
        Some(value) => value.as_millis().min(i32::MAX as u128) as i32,
        None => 0,
    }
}

impl AndroidSocketStream for AndroidSocketStreamImpl {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        unsafe {
            android_jni::to_java_socket_stream_set_read_timeout(
                self.socket_id,
                timeout_to_ms(timeout),
            );
        }
        Ok(())
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        unsafe {
            android_jni::to_java_socket_stream_set_write_timeout(
                self.socket_id,
                timeout_to_ms(timeout),
            );
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let Some(data) =
            (unsafe { android_jni::to_java_socket_stream_read(self.socket_id, buf.len() as i32) })
        else {
            return Ok(0);
        };
        let read_len = data.len().min(buf.len());
        buf[..read_len].copy_from_slice(&data[..read_len]);
        Ok(read_len)
    }

    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let written =
            unsafe { android_jni::to_java_socket_stream_write(self.socket_id, buf.to_vec()) };
        if written < 0 {
            Err(io_other("android platform socket stream write failed"))
        } else {
            Ok(written as usize)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn shutdown(&mut self) {
        if self.is_shutdown {
            return;
        }
        unsafe {
            android_jni::to_java_socket_stream_close(self.socket_id);
        }
        self.is_shutdown = true;
    }
}

impl Drop for AndroidWebSocket {
    fn drop(&mut self) {
        unsafe {
            android_jni::to_java_websocket_close(self.java_socket_id);
        }
    }
}

impl AndroidWebSocket {
    fn open(
        internal_socket_id: LiveId,
        request: HttpRequest,
        rx_sender: Sender<WebSocketMessage>,
    ) -> Self {
        let java_socket_id = LiveId::unique();
        let sender_ref = Arc::new(Box::new((internal_socket_id.0, rx_sender)));
        let pointer = Arc::as_ptr(&sender_ref);
        unsafe {
            android_jni::to_java_websocket_open(java_socket_id, request, pointer);
        }
        Self {
            _sender_ref: sender_ref,
            java_socket_id,
        }
    }

    fn close(&self) {
        unsafe {
            android_jni::to_java_websocket_close(self.java_socket_id);
        }
    }

    fn send_message(&mut self, message: WebSocketMessage) -> Result<(), ()> {
        let frame = match &message {
            WebSocketMessage::String(data) => {
                let header = ServerWebSocketMessageHeader::from_len(
                    data.len(),
                    ServerWebSocketMessageFormat::Text,
                    false,
                );
                WebSocketParser::build_message(header, &data.to_string().into_bytes())
            }
            WebSocketMessage::Binary(data) => {
                let header = ServerWebSocketMessageHeader::from_len(
                    data.len(),
                    ServerWebSocketMessageFormat::Binary,
                    false,
                );
                WebSocketParser::build_message(header, data)
            }
            _ => return Err(()),
        };
        unsafe {
            android_jni::to_java_websocket_send_message(self.java_socket_id, frame);
        }
        Ok(())
    }
}

#[derive(Default)]
struct WsState {
    by_public: HashMap<LiveId, PendingWs>,
    internal_to_public: HashMap<LiveId, LiveId>,
    parsers: HashMap<LiveId, WebSocketParser>,
}

struct AndroidNetworkShimBackend {
    next_internal_id: AtomicU64,
    http: Mutex<HttpState>,
    ws: Mutex<WsState>,
}

impl AndroidNetworkShimBackend {
    fn new() -> Self {
        Self {
            next_internal_id: AtomicU64::new(1),
            http: Mutex::new(HttpState::default()),
            ws: Mutex::new(WsState::default()),
        }
    }

    fn next_internal_id(&self) -> LiveId {
        let raw = self.next_internal_id.fetch_add(1, Ordering::Relaxed) | (1u64 << 63);
        LiveId(raw)
    }

    fn remove_ws_internal(state: &mut WsState, internal_socket_id: LiveId) -> Option<PendingWs> {
        let public_id = state.internal_to_public.remove(&internal_socket_id)?;
        state.parsers.remove(&internal_socket_id);
        state.by_public.remove(&public_id)
    }

    fn handle_http_response(
        &self,
        internal_request_id: LiveId,
        metadata_id: LiveId,
        status_code: u16,
        headers: &str,
        body: &[u8],
    ) -> bool {
        let pending = {
            let mut state = match self.http.lock() {
                Ok(state) => state,
                Err(_) => return false,
            };
            let Some(pending) = state.by_internal.remove(&internal_request_id) else {
                return false;
            };
            state.by_public.remove(&pending.public_request_id);
            pending
        };

        let response = HttpResponse::from_header_string(
            metadata_id,
            status_code,
            headers.to_string(),
            Some(body.to_vec()),
        );
        let _ = pending.sink.emit(NetworkResponse::HttpResponse {
            request_id: pending.public_request_id,
            response,
        });
        true
    }

    fn handle_http_error(
        &self,
        internal_request_id: LiveId,
        metadata_id: LiveId,
        error: &str,
    ) -> bool {
        let pending = {
            let mut state = match self.http.lock() {
                Ok(state) => state,
                Err(_) => return false,
            };
            let Some(pending) = state.by_internal.remove(&internal_request_id) else {
                return false;
            };
            state.by_public.remove(&pending.public_request_id);
            pending
        };

        let _ = pending.sink.emit(NetworkResponse::HttpError {
            request_id: pending.public_request_id,
            error: HttpError {
                message: error.to_string(),
                metadata_id,
            },
        });
        true
    }

    fn handle_ws_message(
        &self,
        internal_socket_id: LiveId,
        message: &[u8],
        sender: &Sender<WebSocketMessage>,
    ) -> bool {
        let mut state = match self.ws.lock() {
            Ok(state) => state,
            Err(_) => return false,
        };
        let Some(parser) = state.parsers.get_mut(&internal_socket_id) else {
            return false;
        };
        parser.parse(message, |result| match result {
            Ok(ServerWebSocketMessage::Text(text)) => {
                let _ = sender.send(WebSocketMessage::String(text.to_string()));
            }
            Ok(ServerWebSocketMessage::Binary(data)) => {
                let _ = sender.send(WebSocketMessage::Binary(data.to_vec()));
            }
            _ => {}
        });
        true
    }

    fn handle_ws_closed(
        &self,
        internal_socket_id: LiveId,
        sender: &Sender<WebSocketMessage>,
    ) -> bool {
        let removed = {
            let mut state = match self.ws.lock() {
                Ok(state) => state,
                Err(_) => return false,
            };
            Self::remove_ws_internal(&mut state, internal_socket_id)
        };
        let Some(_removed) = removed else {
            return false;
        };
        let _ = sender.send(WebSocketMessage::Closed);
        true
    }

    fn handle_ws_error(
        &self,
        internal_socket_id: LiveId,
        error: &str,
        sender: &Sender<WebSocketMessage>,
    ) -> bool {
        let removed = {
            let mut state = match self.ws.lock() {
                Ok(state) => state,
                Err(_) => return false,
            };
            Self::remove_ws_internal(&mut state, internal_socket_id)
        };
        let Some(_removed) = removed else {
            return false;
        };
        let _ = sender.send(WebSocketMessage::Error(error.to_string()));
        true
    }
}

impl NetworkBackend for AndroidNetworkShimBackend {
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
                .map_err(|_| NetworkError::backend("android shim http lock poisoned"))?;
            state.by_public.insert(request_id, internal_request_id);
            state.by_internal.insert(
                internal_request_id,
                PendingHttp {
                    public_request_id: request_id,
                    sink,
                },
            );
        }

        unsafe {
            android_jni::to_java_http_request(internal_request_id, request);
        }
        Ok(())
    }

    fn http_cancel(&self, request_id: LiveId) -> Result<(), NetworkError> {
        let mut state = self
            .http
            .lock()
            .map_err(|_| NetworkError::backend("android shim http lock poisoned"))?;
        if let Some(internal_id) = state.by_public.remove(&request_id) {
            state.by_internal.remove(&internal_id);
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

        let (sender, receiver) = std::sync::mpsc::channel::<WebSocketMessage>();
        let socket = AndroidWebSocket::open(internal_socket_id, request, sender);

        {
            let mut state = self
                .ws
                .lock()
                .map_err(|_| NetworkError::backend("android shim websocket lock poisoned"))?;
            state
                .internal_to_public
                .insert(internal_socket_id, socket_id);
            state
                .parsers
                .insert(internal_socket_id, WebSocketParser::new());
            state.by_public.insert(
                socket_id,
                PendingWs {
                    internal_socket_id,
                    sink: sink.clone(),
                    socket,
                },
            );
        }

        let _ = sink.emit(NetworkResponse::WsOpened { socket_id });
        std::thread::spawn(move || {
            while let Ok(message) = receiver.recv() {
                let event = match message {
                    WebSocketMessage::Opened => NetworkResponse::WsOpened { socket_id },
                    WebSocketMessage::Closed => NetworkResponse::WsClosed { socket_id },
                    WebSocketMessage::String(data) => NetworkResponse::WsMessage {
                        socket_id,
                        message: WsMessage::Text(data),
                    },
                    WebSocketMessage::Binary(data) => NetworkResponse::WsMessage {
                        socket_id,
                        message: WsMessage::Binary(data),
                    },
                    WebSocketMessage::Error(message) => {
                        NetworkResponse::WsError { socket_id, message }
                    }
                };
                if sink.emit(event).is_err() {
                    break;
                }
            }
        });

        Ok(())
    }

    fn ws_send(&self, socket_id: LiveId, message: WsSend) -> Result<(), NetworkError> {
        let mut state = self
            .ws
            .lock()
            .map_err(|_| NetworkError::backend("android shim websocket lock poisoned"))?;
        let pending = state.by_public.get_mut(&socket_id).ok_or_else(|| {
            NetworkError::backend(format!("android shim websocket {socket_id} not open"))
        })?;
        let message = match message {
            WsSend::Binary(data) => WebSocketMessage::Binary(data),
            WsSend::Text(data) => WebSocketMessage::String(data),
        };
        pending
            .socket
            .send_message(message)
            .map_err(|_| NetworkError::backend("android shim websocket send failed"))
    }

    fn ws_close(&self, socket_id: LiveId) -> Result<(), NetworkError> {
        let removed = {
            let mut state = self
                .ws
                .lock()
                .map_err(|_| NetworkError::backend("android shim websocket lock poisoned"))?;
            let Some(pending) = state.by_public.remove(&socket_id) else {
                return Ok(());
            };
            state.internal_to_public.remove(&pending.internal_socket_id);
            state.parsers.remove(&pending.internal_socket_id);
            Some(pending)
        };
        if let Some(pending) = removed {
            pending.socket.close();
            let _ = pending.sink.emit(NetworkResponse::WsClosed { socket_id });
        }
        Ok(())
    }
}

static INIT: Once = Once::new();
static SHIM_BACKEND: OnceLock<Arc<AndroidNetworkShimBackend>> = OnceLock::new();

fn shim_backend() -> Option<&'static Arc<AndroidNetworkShimBackend>> {
    SHIM_BACKEND.get()
}

pub(crate) fn install_network_backend_shim() {
    INIT.call_once(|| {
        let backend = Arc::new(AndroidNetworkShimBackend::new());
        let _ = SHIM_BACKEND.set(backend.clone());
        crate::makepad_network::register_android_backend_shim(backend);
        crate::makepad_network::register_android_socket_stream_factory_shim(Arc::new(
            AndroidSocketStreamFactoryImpl,
        ));
    });
}

pub(crate) fn try_handle_http_response(
    internal_request_id: LiveId,
    metadata_id: LiveId,
    status_code: u16,
    headers: &str,
    body: &[u8],
) -> bool {
    let Some(backend) = shim_backend() else {
        return false;
    };
    backend.handle_http_response(internal_request_id, metadata_id, status_code, headers, body)
}

pub(crate) fn try_handle_http_error(
    internal_request_id: LiveId,
    metadata_id: LiveId,
    error: &str,
) -> bool {
    let Some(backend) = shim_backend() else {
        return false;
    };
    backend.handle_http_error(internal_request_id, metadata_id, error)
}

pub(crate) fn try_handle_websocket_message(
    internal_socket_id: u64,
    message: &[u8],
    sender: &Sender<WebSocketMessage>,
) -> bool {
    let Some(backend) = shim_backend() else {
        return false;
    };
    backend.handle_ws_message(LiveId(internal_socket_id), message, sender)
}

pub(crate) fn try_handle_websocket_closed(
    internal_socket_id: u64,
    sender: &Sender<WebSocketMessage>,
) -> bool {
    let Some(backend) = shim_backend() else {
        return false;
    };
    backend.handle_ws_closed(LiveId(internal_socket_id), sender)
}

pub(crate) fn try_handle_websocket_error(
    internal_socket_id: u64,
    error: &str,
    sender: &Sender<WebSocketMessage>,
) -> bool {
    let Some(backend) = shim_backend() else {
        return false;
    };
    backend.handle_ws_error(LiveId(internal_socket_id), error, sender)
}
