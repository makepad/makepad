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
    clear_platform_socket_factory, register_platform_socket_factory, PlatformSocketFactory, PlatformSocketStream,
};
#[cfg(any(target_os = "ios", target_os = "macos", target_os = "tvos"))]
pub mod apple;
#[cfg(any(target_os = "linux", target_os = "android"))]
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
    /// A websocket the platform's own event loop reads (a HAL hosted
    /// child's host socket): its events wake the loop, never the app.
    loop_socket: Arc<Mutex<Option<LiveId>>>,
}

/// What an event of the sink does besides being queued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delivery {
    /// Queued only: the reader is blocked on the runtime.
    Silent,
    /// Wakes the event loop (the runtime's wake fn): the platform reads it.
    WakeLoop,
    /// Wakes the loop and raises the UI signal: the app polls for it.
    Signal,
}

fn delivery(event: &NetworkResponse, quiet: Option<LiveId>, own: Option<LiveId>) -> Delivery {
    let socket = match event {
        NetworkResponse::WsOpened { socket_id }
        | NetworkResponse::WsMessage { socket_id, .. }
        | NetworkResponse::WsClosed { socket_id }
        | NetworkResponse::WsError { socket_id, .. } => Some(*socket_id),
        _ => None,
    };
    match socket {
        Some(socket) if quiet == Some(socket) => Delivery::Silent,
        Some(socket) if own == Some(socket) => Delivery::WakeLoop,
        _ => Delivery::Signal,
    }
}

impl EventSink {
    pub(crate) fn new(sender: Sender<NetworkResponse>) -> Self {
        Self {
            sender,
            wake_fn: Arc::new(Mutex::new(None)),
            quiet_socket: Arc::new(Mutex::new(None)),
            loop_socket: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn set_loop_socket(&self, socket_id: Option<LiveId>) {
        if let Ok(mut guard) = self.loop_socket.lock() {
            *guard = socket_id;
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
        let quiet = self.quiet_socket.lock().ok().and_then(|guard| *guard);
        let own = self.loop_socket.lock().ok().and_then(|guard| *guard);
        let delivery = delivery(&event, quiet, own);
        self.sender
            .send(event)
            .map_err(|_| NetworkError::ChannelClosed)?;
        if delivery == Delivery::Silent {
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
        if delivery == Delivery::WakeLoop {
            // The loop reads it; an `Event::Signal` to every widget for each
            // host message (a hosted Stage: 100+ a second) would be all waste.
            // The sink of a loop socket always has the platform's wake fn.
            return Ok(());
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

#[cfg(all(
    not(target_arch = "wasm32"),
    target_os = "windows"
))]
pub fn default_backend() -> Arc<dyn NetworkBackend> {
    windows::create_backend()
}

#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "windows"),
    any(target_os = "ios", target_os = "macos", target_os = "tvos")
))]
pub fn default_backend() -> Arc<dyn NetworkBackend> {
    apple::create_backend()
}

#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "windows"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    any(target_os = "linux", target_os = "android")
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

    #[test]
    fn the_loop_socket_wakes_the_loop_but_raises_no_ui_signal() {
        let own = Some(LiveId(0));
        let message = |id| NetworkResponse::WsMessage { socket_id: LiveId(id), message: crate::types::WsMessage::Binary(Vec::new()) };
        assert_eq!(delivery(&message(0), None, own), Delivery::WakeLoop);
        assert_eq!(delivery(&NetworkResponse::WsOpened { socket_id: LiveId(0) }, None, own), Delivery::WakeLoop);
        // the app's own sockets and requests still signal the app
        assert_eq!(delivery(&message(5), None, own), Delivery::Signal);
        assert_eq!(delivery(&message(0), None, None), Delivery::Signal);
        assert_eq!(delivery(&message(0), Some(LiveId(0)), own), Delivery::Silent);
    }

    #[test]
    fn loop_socket_events_call_the_wake_fn() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let sink = EventSink::new(sender);
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        sink.set_wake_fn(Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })));
        sink.set_loop_socket(Some(LiveId(3)));
        sink.emit(NetworkResponse::WsOpened { socket_id: LiveId(3) }).unwrap();
        assert!(receiver.try_recv().is_ok());
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }
}
