use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use crate::types::{
    HttpRequest, NetworkError, NetworkEvent, RequestId, SocketId, WsOpenRequest, WsSend,
};

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos", target_os = "tvos"))]
pub mod apple;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_os = "windows")]
pub mod windows;

pub type WakeFn = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct EventSink {
    sender: Sender<NetworkEvent>,
    wake_fn: Arc<Mutex<Option<WakeFn>>>,
}

impl EventSink {
    pub(crate) fn new(sender: Sender<NetworkEvent>) -> Self {
        Self {
            sender,
            wake_fn: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn set_wake_fn(&self, wake_fn: Option<WakeFn>) {
        if let Ok(mut guard) = self.wake_fn.lock() {
            *guard = wake_fn;
        }
    }

    pub fn emit(&self, event: NetworkEvent) -> Result<(), NetworkError> {
        self.sender
            .send(event)
            .map_err(|_| NetworkError::ChannelClosed)?;

        let wake_fn = self
            .wake_fn
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(Arc::clone));
        if let Some(wake_fn) = wake_fn {
            wake_fn();
        }
        Ok(())
    }
}

pub trait NetworkBackend: Send + Sync + 'static {
    fn http_start(
        &self,
        request_id: RequestId,
        request: HttpRequest,
        sink: EventSink,
    ) -> Result<(), NetworkError>;

    fn http_cancel(&self, request_id: RequestId) -> Result<(), NetworkError>;

    fn ws_open(
        &self,
        socket_id: SocketId,
        request: WsOpenRequest,
        sink: EventSink,
    ) -> Result<(), NetworkError>;

    fn ws_send(&self, socket_id: SocketId, message: WsSend) -> Result<(), NetworkError>;

    fn ws_close(&self, socket_id: SocketId) -> Result<(), NetworkError>;
}

#[derive(Default)]
pub struct UnsupportedBackend {
    reason: &'static str,
}

impl UnsupportedBackend {
    pub fn new(reason: &'static str) -> Self {
        Self { reason }
    }

    fn unsupported(&self) -> NetworkError {
        let reason = if self.reason.is_empty() {
            "no backend configured"
        } else {
            self.reason
        };
        NetworkError::Unsupported(reason)
    }
}

impl NetworkBackend for UnsupportedBackend {
    fn http_start(
        &self,
        _request_id: RequestId,
        _request: HttpRequest,
        _sink: EventSink,
    ) -> Result<(), NetworkError> {
        Err(self.unsupported())
    }

    fn http_cancel(&self, _request_id: RequestId) -> Result<(), NetworkError> {
        Err(self.unsupported())
    }

    fn ws_open(
        &self,
        _socket_id: SocketId,
        _request: WsOpenRequest,
        _sink: EventSink,
    ) -> Result<(), NetworkError> {
        Err(self.unsupported())
    }

    fn ws_send(&self, _socket_id: SocketId, _message: WsSend) -> Result<(), NetworkError> {
        Err(self.unsupported())
    }

    fn ws_close(&self, _socket_id: SocketId) -> Result<(), NetworkError> {
        Err(self.unsupported())
    }
}

#[cfg(target_arch = "wasm32")]
pub fn default_backend() -> Arc<dyn NetworkBackend> {
    web::create_backend()
}

#[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
pub fn default_backend() -> Arc<dyn NetworkBackend> {
    android::create_backend()
}

#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "android"),
    target_os = "windows"
))]
pub fn default_backend() -> Arc<dyn NetworkBackend> {
    windows::create_backend()
}

#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "android"),
    not(target_os = "windows"),
    any(target_os = "ios", target_os = "macos", target_os = "tvos")
))]
pub fn default_backend() -> Arc<dyn NetworkBackend> {
    apple::create_backend()
}

#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "android"),
    not(target_os = "windows"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    target_os = "linux"
))]
pub fn default_backend() -> Arc<dyn NetworkBackend> {
    linux::create_backend()
}

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "windows",
    target_os = "linux",
    target_os = "ios",
    target_os = "macos",
    target_os = "tvos"
)))]
pub fn default_backend() -> Arc<dyn NetworkBackend> {
    Arc::new(UnsupportedBackend::new(
        "no default backend implemented for this target",
    ))
}
