use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use makepad_live_id::LiveId;

use crate::types::{HttpRequest, NetworkError, NetworkResponse, WsSend};
use crate::ui_signal::SignalToUI;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub(crate) use self::android::connect_platform_socket_stream;
#[cfg(target_os = "android")]
pub use self::android::{
    clear_platform_backend, clear_platform_socket_factory, register_platform_backend,
    register_platform_socket_factory, PlatformSocketFactory, PlatformSocketStream,
};
#[cfg(any(target_os = "ios", target_os = "macos", target_os = "tvos"))]
pub mod apple;
#[cfg(target_os = "linux")]
// Linux socket workers are native-only and never compiled into a web app.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod linux;
#[cfg(target_arch = "wasm32")]
pub mod web;
#[cfg(target_os = "windows")]
pub mod windows;

#[derive(Clone)]
pub struct EventSink {
    sender: Sender<NetworkResponse>,
    wake_fn: Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>,
    /// A websocket whose events are read by a loop blocked on this runtime
    /// (a hosted child's host socket): they neither wake nor signal the UI.
    quiet_socket: Arc<Mutex<Option<LiveId>>>,
}

impl EventSink {
    pub(crate) fn new(sender: Sender<NetworkResponse>) -> Self {
        Self {
            sender,
            wake_fn: Arc::new(Mutex::new(None)),
            quiet_socket: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn set_quiet_socket(&self, socket_id: Option<LiveId>) {
        if let Ok(mut guard) = self.quiet_socket.lock() {
            *guard = socket_id;
        }
    }

    pub(crate) fn set_wake_fn(&self, wake_fn: Option<Arc<dyn Fn() + Send + Sync>>) {
        if let Ok(mut guard) = self.wake_fn.lock() {
            *guard = wake_fn;
        }
    }

    pub fn emit(&self, event: NetworkResponse) -> Result<(), NetworkError> {
        let quiet = match &event {
            NetworkResponse::WsOpened { socket_id }
            | NetworkResponse::WsMessage { socket_id, .. }
            | NetworkResponse::WsClosed { socket_id }
            | NetworkResponse::WsError { socket_id, .. } => {
                self.quiet_socket.lock().ok().is_some_and(|guard| *guard == Some(*socket_id))
            }
            _ => false,
        };
        self.sender
            .send(event)
            .map_err(|_| NetworkError::ChannelClosed)?;
        if quiet {
            return Ok(());
        }

        let wake_fn = self
            .wake_fn
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(Arc::clone));
        if let Some(wake_fn) = wake_fn {
            wake_fn();
        }
        // Every completion that can unblock work on the UI thread raises the
        // UI signal: a runtime the platform did not create (the asset client
        // builds its own) has no wake fn, and on wasm nothing else runs the
        // event loop until an unrelated timer fires.
        SignalToUI::set_ui_signal();
        Ok(())
    }
}

pub trait NetworkBackend: Send + Sync + 'static {
    fn http_start(
        &self,
        request_id: LiveId,
        request: HttpRequest,
        sink: EventSink,
    ) -> Result<(), NetworkError>;

    fn http_cancel(&self, request_id: LiveId) -> Result<(), NetworkError>;

    fn ws_open(
        &self,
        socket_id: LiveId,
        request: HttpRequest,
        sink: EventSink,
    ) -> Result<(), NetworkError>;

    fn ws_send(&self, socket_id: LiveId, message: WsSend) -> Result<(), NetworkError>;

    fn ws_close(&self, socket_id: LiveId) -> Result<(), NetworkError>;
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
        _request_id: LiveId,
        _request: HttpRequest,
        _sink: EventSink,
    ) -> Result<(), NetworkError> {
        Err(self.unsupported())
    }

    fn http_cancel(&self, _request_id: LiveId) -> Result<(), NetworkError> {
        Err(self.unsupported())
    }

    fn ws_open(
        &self,
        _socket_id: LiveId,
        _request: HttpRequest,
        _sink: EventSink,
    ) -> Result<(), NetworkError> {
        Err(self.unsupported())
    }

    fn ws_send(&self, _socket_id: LiveId, _message: WsSend) -> Result<(), NetworkError> {
        Err(self.unsupported())
    }

    fn ws_close(&self, _socket_id: LiveId) -> Result<(), NetworkError> {
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

#[cfg(test)]
mod quiet_socket_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn quiet_socket_events_are_delivered_without_waking() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let sink = EventSink::new(sender);
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        sink.set_wake_fn(Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })));
        sink.set_quiet_socket(Some(LiveId(7)));
        sink.emit(NetworkResponse::WsOpened { socket_id: LiveId(7) }).unwrap();
        assert!(receiver.try_recv().is_ok());
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
        sink.emit(NetworkResponse::WsOpened { socket_id: LiveId(8) }).unwrap();
        assert!(receiver.try_recv().is_ok());
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }
}
