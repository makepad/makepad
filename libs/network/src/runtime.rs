use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::backend::{default_backend, EventSink, NetworkBackend, WakeFn};
use crate::types::{
    HttpRequest, NetworkError, NetworkEvent, RequestId, SocketId, WsOpenRequest, WsSend,
};

#[derive(Default)]
pub struct NetworkConfig {
    pub backend: Option<Arc<dyn NetworkBackend>>,
}

pub struct NetworkRuntime {
    backend: Arc<dyn NetworkBackend>,
    sink: EventSink,
    receiver: Mutex<Receiver<NetworkEvent>>,
}

impl NetworkRuntime {
    pub fn new(config: NetworkConfig) -> Self {
        let backend = config.backend.unwrap_or_else(default_backend);
        Self::with_backend(backend)
    }

    pub fn with_backend(backend: Arc<dyn NetworkBackend>) -> Self {
        let (sender, receiver) = channel();
        Self {
            backend,
            sink: EventSink::new(sender),
            receiver: Mutex::new(receiver),
        }
    }

    pub fn set_wake_fn(&self, wake_fn: Option<WakeFn>) {
        self.sink.set_wake_fn(wake_fn);
    }

    pub fn http_start(
        &self,
        request_id: RequestId,
        request: HttpRequest,
    ) -> Result<(), NetworkError> {
        self.backend
            .http_start(request_id, request, self.sink.clone())
    }

    pub fn http_cancel(&self, request_id: RequestId) -> Result<(), NetworkError> {
        self.backend.http_cancel(request_id)
    }

    pub fn ws_open(&self, socket_id: SocketId, request: WsOpenRequest) -> Result<(), NetworkError> {
        self.backend.ws_open(socket_id, request, self.sink.clone())
    }

    pub fn ws_send(&self, socket_id: SocketId, message: WsSend) -> Result<(), NetworkError> {
        self.backend.ws_send(socket_id, message)
    }

    pub fn ws_close(&self, socket_id: SocketId) -> Result<(), NetworkError> {
        self.backend.ws_close(socket_id)
    }

    pub fn try_recv(&self) -> Option<NetworkEvent> {
        self.receiver.lock().ok()?.try_recv().ok()
    }

    pub fn recv(&self) -> Result<NetworkEvent, NetworkError> {
        self.receiver
            .lock()
            .map_err(|_| NetworkError::ChannelClosed)?
            .recv()
            .map_err(|_| NetworkError::ChannelClosed)
    }

    pub fn recv_timeout(&self, duration: Duration) -> Option<NetworkEvent> {
        self.receiver.lock().ok()?.recv_timeout(duration).ok()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use crate::backend::{EventSink, NetworkBackend};
    use crate::types::{
        Headers, HttpMethod, HttpRequest, HttpResponse, NetworkEvent, RequestId, SocketId,
        WsOpenRequest, WsSend,
    };

    use super::NetworkRuntime;

    struct TestBackend;

    impl NetworkBackend for TestBackend {
        fn http_start(
            &self,
            request_id: RequestId,
            _request: HttpRequest,
            sink: EventSink,
        ) -> Result<(), crate::types::NetworkError> {
            let response = HttpResponse::new(42, 200, Headers::new(), Some(b"ok".to_vec()));
            sink.emit(NetworkEvent::HttpResponse {
                request_id,
                response,
            })
        }

        fn http_cancel(&self, _request_id: RequestId) -> Result<(), crate::types::NetworkError> {
            Ok(())
        }

        fn ws_open(
            &self,
            _socket_id: SocketId,
            _request: WsOpenRequest,
            _sink: EventSink,
        ) -> Result<(), crate::types::NetworkError> {
            Ok(())
        }

        fn ws_send(
            &self,
            _socket_id: SocketId,
            _message: WsSend,
        ) -> Result<(), crate::types::NetworkError> {
            Ok(())
        }

        fn ws_close(&self, _socket_id: SocketId) -> Result<(), crate::types::NetworkError> {
            Ok(())
        }
    }

    #[test]
    fn runtime_supports_headless_queue_and_wake_fn() {
        let runtime = NetworkRuntime::with_backend(Arc::new(TestBackend));
        let wakes = Arc::new(AtomicUsize::new(0));
        let wakes_clone = Arc::clone(&wakes);

        runtime.set_wake_fn(Some(Arc::new(move || {
            wakes_clone.fetch_add(1, Ordering::SeqCst);
        })));

        let request = HttpRequest::new("https://example.com".to_string(), HttpMethod::Get);
        runtime.http_start(7, request).unwrap();

        let event = runtime.recv_timeout(Duration::from_millis(50)).unwrap();
        match event {
            NetworkEvent::HttpResponse {
                request_id,
                response,
            } => {
                assert_eq!(request_id, 7);
                assert_eq!(response.status_code, 200);
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }
}
