use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use makepad_live_id::LiveId;

use crate::backend::{default_backend, EventSink, NetworkBackend};
use crate::http_server::HttpServer;
use crate::types::{HttpRequest, NetworkError, NetworkResponse, WsSend};

#[derive(Default)]
pub struct NetworkConfig {
    pub backend: Option<Arc<dyn NetworkBackend>>,
}

pub struct NetworkRuntime {
    backend: Arc<dyn NetworkBackend>,
    sink: EventSink,
    receiver: Mutex<Receiver<NetworkResponse>>,
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

    pub fn set_wake_fn(&self, wake_fn: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.sink.set_wake_fn(wake_fn);
    }

    pub fn http_start(&self, request_id: LiveId, request: HttpRequest) -> Result<(), NetworkError> {
        self.backend
            .http_start(request_id, request, self.sink.clone())
    }

    pub fn http_cancel(&self, request_id: LiveId) -> Result<(), NetworkError> {
        self.backend.http_cancel(request_id)
    }

    pub fn ws_open(&self, socket_id: LiveId, request: HttpRequest) -> Result<(), NetworkError> {
        self.backend.ws_open(socket_id, request, self.sink.clone())
    }

    pub fn ws_send(&self, socket_id: LiveId, message: WsSend) -> Result<(), NetworkError> {
        self.backend.ws_send(socket_id, message)
    }

    pub fn ws_close(&self, socket_id: LiveId) -> Result<(), NetworkError> {
        self.backend.ws_close(socket_id)
    }

    pub fn start_http_server(
        &self,
        http_server: HttpServer,
    ) -> Option<std::thread::JoinHandle<()>> {
        crate::http_server::start_http_server(http_server)
    }

    pub fn try_recv(&self) -> Option<NetworkResponse> {
        self.receiver.lock().ok()?.try_recv().ok()
    }

    pub fn recv(&self) -> Result<NetworkResponse, NetworkError> {
        self.receiver
            .lock()
            .map_err(|_| NetworkError::ChannelClosed)?
            .recv()
            .map_err(|_| NetworkError::ChannelClosed)
    }

    pub fn recv_timeout(&self, duration: Duration) -> Option<NetworkResponse> {
        self.receiver.lock().ok()?.recv_timeout(duration).ok()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use crate::backend::{EventSink, NetworkBackend};
    use crate::types::{HttpMethod, HttpRequest, HttpResponse, NetworkResponse, WsSend};
    use makepad_live_id::LiveId;

    use super::NetworkRuntime;

    struct TestBackend;

    impl NetworkBackend for TestBackend {
        fn http_start(
            &self,
            request_id: LiveId,
            _request: HttpRequest,
            sink: EventSink,
        ) -> Result<(), crate::types::NetworkError> {
            let response =
                HttpResponse::new(LiveId(42), 200, BTreeMap::new(), Some(b"ok".to_vec()));
            sink.emit(NetworkResponse::HttpResponse {
                request_id,
                response,
            })
        }

        fn http_cancel(&self, _request_id: LiveId) -> Result<(), crate::types::NetworkError> {
            Ok(())
        }

        fn ws_open(
            &self,
            _socket_id: LiveId,
            _request: HttpRequest,
            _sink: EventSink,
        ) -> Result<(), crate::types::NetworkError> {
            Ok(())
        }

        fn ws_send(
            &self,
            _socket_id: LiveId,
            _message: WsSend,
        ) -> Result<(), crate::types::NetworkError> {
            Ok(())
        }

        fn ws_close(&self, _socket_id: LiveId) -> Result<(), crate::types::NetworkError> {
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

        let request = HttpRequest::new("https://example.com".to_string(), HttpMethod::GET);
        runtime.http_start(LiveId(7), request).unwrap();

        let event = runtime.recv_timeout(Duration::from_millis(50)).unwrap();
        match event {
            NetworkResponse::HttpResponse {
                request_id,
                response,
            } => {
                assert_eq!(request_id, LiveId(7));
                assert_eq!(response.status_code, 200);
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }
}
