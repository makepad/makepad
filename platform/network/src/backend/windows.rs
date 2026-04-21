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

enum WindowsSocket {
    Plain(crate::plain_web_socket::PlainWebSocket),
    Platform(self::web_socket::WindowsWebSocket),
}

pub(crate) struct WindowsBackend {
    sockets: Mutex<HashMap<LiveId, WindowsSocket>>,
}

impl WindowsBackend {
    fn new() -> Self {
        Self {
            sockets: Mutex::new(HashMap::new()),
        }
    }
}

impl NetworkBackend for WindowsBackend {
    fn http_start(
        &self,
        request_id: LiveId,
        request: HttpRequest,
        sink: EventSink,
    ) -> Result<(), NetworkError> {
        let (sender, receiver) = std::sync::mpsc::channel::<NetworkResponse>();
        self::http::WindowsHttpSocket::open(request_id, request, sender);

        std::thread::spawn(move || {
            while let Ok(item) = receiver.recv() {
                if sink.emit(item).is_err() {
                    break;
                }
            }
        });
        Ok(())
    }

    fn http_cancel(&self, _request_id: LiveId) -> Result<(), NetworkError> {
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
            WindowsSocket::Plain(crate::plain_web_socket::PlainWebSocket::open(
                socket_id, request, sender,
            ))
        } else {
            WindowsSocket::Platform(self::web_socket::WindowsWebSocket::open(
                socket_id, request, sender,
            ))
        };

        {
            let mut sockets = self
                .sockets
                .lock()
                .map_err(|_| NetworkError::backend("windows websocket lock poisoned"))?;
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
            .map_err(|_| NetworkError::backend("windows websocket lock poisoned"))?;
        let socket = sockets.get_mut(&socket_id).ok_or_else(|| {
            NetworkError::backend(format!("windows websocket {socket_id} not open"))
        })?;
        let outbound = match message {
            WsSend::Binary(data) => WebSocketMessage::Binary(data),
            WsSend::Text(data) => WebSocketMessage::String(data),
        };
        match socket {
            WindowsSocket::Plain(socket) => socket
                .send_message(outbound)
                .map_err(|_| NetworkError::backend("windows websocket send failed")),
            WindowsSocket::Platform(socket) => socket
                .send_message(outbound)
                .map_err(|_| NetworkError::backend("windows websocket send failed")),
        }
    }

    fn ws_close(&self, socket_id: LiveId) -> Result<(), NetworkError> {
        let mut sockets = self
            .sockets
            .lock()
            .map_err(|_| NetworkError::backend("windows websocket lock poisoned"))?;
        if let Some(socket) = sockets.remove(&socket_id) {
            match socket {
                WindowsSocket::Plain(mut socket) => socket.close(),
                WindowsSocket::Platform(mut socket) => socket.close(),
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
    Arc::new(WindowsBackend::new())
}
