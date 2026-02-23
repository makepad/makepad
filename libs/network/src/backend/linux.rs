use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::backend::{EventSink, NetworkBackend};
use crate::types::{
    Headers, HttpRequest, NetworkError, NetworkEvent, NetworkResponse, NetworkResponseItem,
    RequestId, SocketId, WebSocketMessage as LegacyWsMessage, WsMessage, WsOpenRequest, WsSend,
};

pub mod http;
mod socket_stream;
pub mod web_socket;

pub(crate) struct LinuxBackend {
    sockets: Mutex<HashMap<SocketId, self::web_socket::OsWebSocket>>,
}

impl LinuxBackend {
    fn new() -> Self {
        Self {
            sockets: Mutex::new(HashMap::new()),
        }
    }
}

impl NetworkBackend for LinuxBackend {
    fn http_start(
        &self,
        request_id: RequestId,
        request: HttpRequest,
        sink: EventSink,
    ) -> Result<(), NetworkError> {
        let (sender, receiver) = std::sync::mpsc::channel::<NetworkResponseItem>();
        self::http::LinuxHttpSocket::open(request_id, request, sender);

        std::thread::spawn(move || {
            while let Ok(item) = receiver.recv() {
                if sink.emit(map_http_event(item)).is_err() {
                    break;
                }
            }
        });
        Ok(())
    }

    fn http_cancel(&self, request_id: RequestId) -> Result<(), NetworkError> {
        self::http::LinuxHttpSocket::cancel(request_id);
        Ok(())
    }

    fn ws_open(
        &self,
        socket_id: SocketId,
        request: WsOpenRequest,
        sink: EventSink,
    ) -> Result<(), NetworkError> {
        let mut headers = Headers::new();
        headers.extend(request.headers);
        let request = HttpRequest {
            metadata_id: 0,
            url: request.url,
            method: crate::types::HttpMethod::Get,
            headers,
            ignore_ssl_cert: false,
            is_streaming: false,
            body: None,
        };

        let (sender, receiver) = std::sync::mpsc::channel::<LegacyWsMessage>();
        let socket = self::web_socket::OsWebSocket::open(socket_id, request, sender);

        {
            let mut sockets = self
                .sockets
                .lock()
                .map_err(|_| NetworkError::backend("linux websocket lock poisoned"))?;
            sockets.insert(socket_id, socket);
        }

        let _ = sink.emit(NetworkEvent::WsOpened { socket_id });
        std::thread::spawn(move || {
            while let Ok(message) = receiver.recv() {
                if sink.emit(map_ws_event(socket_id, message)).is_err() {
                    break;
                }
            }
        });
        Ok(())
    }

    fn ws_send(&self, socket_id: SocketId, message: WsSend) -> Result<(), NetworkError> {
        let mut sockets = self
            .sockets
            .lock()
            .map_err(|_| NetworkError::backend("linux websocket lock poisoned"))?;
        let socket = sockets.get_mut(&socket_id).ok_or_else(|| {
            NetworkError::backend(format!("linux websocket {socket_id} not open"))
        })?;
        let legacy = match message {
            WsSend::Binary(data) => LegacyWsMessage::Binary(data),
            WsSend::Text(data) => LegacyWsMessage::String(data),
        };
        socket
            .send_message(legacy)
            .map_err(|_| NetworkError::backend("linux websocket send failed"))
    }

    fn ws_close(&self, socket_id: SocketId) -> Result<(), NetworkError> {
        let mut sockets = self
            .sockets
            .lock()
            .map_err(|_| NetworkError::backend("linux websocket lock poisoned"))?;
        if let Some(mut socket) = sockets.remove(&socket_id) {
            socket.close();
        }
        Ok(())
    }
}

fn map_http_event(item: NetworkResponseItem) -> NetworkEvent {
    match item.response {
        NetworkResponse::HttpRequestError(error) => NetworkEvent::HttpError {
            request_id: item.request_id,
            error,
        },
        NetworkResponse::HttpResponse(response) => NetworkEvent::HttpResponse {
            request_id: item.request_id,
            response,
        },
        NetworkResponse::HttpStreamResponse(response) => NetworkEvent::HttpStreamChunk {
            request_id: item.request_id,
            response,
        },
        NetworkResponse::HttpStreamComplete(response) => NetworkEvent::HttpStreamComplete {
            request_id: item.request_id,
            response,
        },
        NetworkResponse::HttpProgress(progress) => NetworkEvent::HttpProgress {
            request_id: item.request_id,
            progress,
        },
    }
}

fn map_ws_event(socket_id: SocketId, message: LegacyWsMessage) -> NetworkEvent {
    match message {
        LegacyWsMessage::Error(message) => NetworkEvent::WsError { socket_id, message },
        LegacyWsMessage::Binary(data) => NetworkEvent::WsMessage {
            socket_id,
            message: WsMessage::Binary(data),
        },
        LegacyWsMessage::String(data) => NetworkEvent::WsMessage {
            socket_id,
            message: WsMessage::Text(data),
        },
        LegacyWsMessage::Opened => NetworkEvent::WsOpened { socket_id },
        LegacyWsMessage::Closed => NetworkEvent::WsClosed { socket_id },
    }
}

pub(crate) fn create_backend() -> Arc<dyn NetworkBackend> {
    Arc::new(LinuxBackend::new())
}
