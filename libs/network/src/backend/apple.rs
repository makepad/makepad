use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use makepad_live_id::LiveId;

use crate::backend::{EventSink, NetworkBackend};
use crate::types::{
    HttpRequest, NetworkError, NetworkResponse, WebSocketMessage, WebSocketTransport, WsMessage,
    WsSend,
};

pub mod http;
pub mod web_socket;

enum AppleSocket {
    Plain(crate::plain_web_socket::PlainWebSocket),
    Platform(self::web_socket::AppleWebSocket),
}

pub(crate) struct AppleBackend {
    http_requests: Arc<Mutex<self::http::AppleHttpRequests>>,
    sockets: Mutex<HashMap<LiveId, AppleSocket>>,
}

impl AppleBackend {
    fn new() -> Self {
        Self {
            http_requests: Arc::new(Mutex::new(self::http::AppleHttpRequests::default())),
            sockets: Mutex::new(HashMap::new()),
        }
    }
}

impl NetworkBackend for AppleBackend {
    fn http_start(
        &self,
        request_id: LiveId,
        request: HttpRequest,
        sink: EventSink,
    ) -> Result<(), NetworkError> {
        let (sender, receiver) = std::sync::mpsc::channel::<NetworkResponse>();
        {
            let mut http_requests = self
                .http_requests
                .lock()
                .map_err(|_| NetworkError::backend("apple http lock poisoned"))?;
            http_requests.make_http_request(request_id, request, sender);
        }

        let http_requests = Arc::clone(&self.http_requests);
        std::thread::spawn(move || {
            while let Ok(response) = receiver.recv() {
                if let Ok(mut requests) = http_requests.lock() {
                    requests.handle_response(&response);
                }
                if sink.emit(response).is_err() {
                    break;
                }
            }
        });
        Ok(())
    }

    fn http_cancel(&self, request_id: LiveId) -> Result<(), NetworkError> {
        let mut requests = self
            .http_requests
            .lock()
            .map_err(|_| NetworkError::backend("apple http lock poisoned"))?;
        requests.cancel_http_request(request_id);
        Ok(())
    }

    fn ws_open(
        &self,
        socket_id: LiveId,
        request: HttpRequest,
        sink: EventSink,
    ) -> Result<(), NetworkError> {
        let use_plain = matches!(request.websocket_transport, WebSocketTransport::PlainTcp);

        let (sender, receiver) = std::sync::mpsc::channel::<WebSocketMessage>();
        let socket = if use_plain {
            AppleSocket::Plain(crate::plain_web_socket::PlainWebSocket::open(
                socket_id, request, sender,
            ))
        } else {
            AppleSocket::Platform(self::web_socket::AppleWebSocket::open(
                socket_id, request, sender,
            ))
        };

        {
            let mut sockets = self
                .sockets
                .lock()
                .map_err(|_| NetworkError::backend("apple websocket lock poisoned"))?;
            sockets.insert(socket_id, socket);
        }

        let _ = sink.emit(NetworkResponse::WsOpened { socket_id });
        std::thread::spawn(move || {
            while let Ok(message) = receiver.recv() {
                if sink.emit(map_ws_event(socket_id, message)).is_err() {
                    break;
                }
            }
        });
        Ok(())
    }

    fn ws_send(&self, socket_id: LiveId, message: WsSend) -> Result<(), NetworkError> {
        let mut sockets = self
            .sockets
            .lock()
            .map_err(|_| NetworkError::backend("apple websocket lock poisoned"))?;
        let socket = sockets.get_mut(&socket_id).ok_or_else(|| {
            NetworkError::backend(format!("apple websocket {socket_id} not open"))
        })?;
        let outbound = match message {
            WsSend::Binary(data) => WebSocketMessage::Binary(data),
            WsSend::Text(data) => WebSocketMessage::String(data),
        };
        match socket {
            AppleSocket::Plain(socket) => socket
                .send_message(outbound)
                .map_err(|_| NetworkError::backend("apple websocket send failed")),
            AppleSocket::Platform(socket) => socket
                .send_message(outbound)
                .map_err(|_| NetworkError::backend("apple websocket send failed")),
        }
    }

    fn ws_close(&self, socket_id: LiveId) -> Result<(), NetworkError> {
        let mut sockets = self
            .sockets
            .lock()
            .map_err(|_| NetworkError::backend("apple websocket lock poisoned"))?;
        if let Some(socket) = sockets.remove(&socket_id) {
            #[allow(unused_mut)]
            match socket {
                AppleSocket::Plain(mut socket) => socket.close(),
                AppleSocket::Platform(mut socket) => socket.close(),
            }
        }
        Ok(())
    }
}

fn map_ws_event(socket_id: LiveId, message: WebSocketMessage) -> NetworkResponse {
    match message {
        WebSocketMessage::Error(message) => NetworkResponse::WsError { socket_id, message },
        WebSocketMessage::Binary(data) => NetworkResponse::WsMessage {
            socket_id,
            message: WsMessage::Binary(data),
        },
        WebSocketMessage::String(data) => NetworkResponse::WsMessage {
            socket_id,
            message: WsMessage::Text(data),
        },
        WebSocketMessage::Opened => NetworkResponse::WsOpened { socket_id },
        WebSocketMessage::Closed => NetworkResponse::WsClosed { socket_id },
    }
}

pub(crate) fn create_backend() -> Arc<dyn NetworkBackend> {
    Arc::new(AppleBackend::new())
}
