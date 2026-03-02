#[allow(unused_imports)]
use crate::{
    cx_api::*,
    event::Event,
    makepad_micro_serde::*,
    makepad_network::{
        HttpMethod, HttpRequest, NetworkResponse, NetworkRuntime, WebSocketTransport, WsMessage,
        WsSend,
    },
    thread::SignalToUI,
    makepad_live_id::LiveId,
    Cx,
};
use makepad_studio_protocol::{AppToStudio, AppToStudioVec, LocalProfileSample, StudioToApp};
pub use crate::makepad_network::WebSocketMessage;
#[allow(unused_imports)]
use std::{
    sync::Arc,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError},
        Mutex,
    },
    time::Duration,
};

pub type WebSocket = u64;

#[derive(Debug)]
enum StudioWebSocketThreadMsg {
    AppToStudio {
        message: AppToStudio,
    },
    Terminate,
}

static STUDIO_WEB_SOCKET_THREAD_SENDER: Mutex<Option<Sender<StudioWebSocketThreadMsg>>> =
    Mutex::new(None);
static STUDIO_NET_RUNTIME: Mutex<Option<Arc<NetworkRuntime>>> = Mutex::new(None);
pub(crate) static HAS_STUDIO_WEB_SOCKET: AtomicBool = AtomicBool::new(false);
pub(crate) static STUDIO_STDOUT_MODE: AtomicBool = AtomicBool::new(false);
pub(crate) static LOCAL_PROFILE_CAPTURE_ENABLED: AtomicBool = AtomicBool::new(false);
pub(crate) static CONTROL_CHANNEL: Mutex<Option<Receiver<StudioToApp>>> =
    Mutex::new(None);
pub(crate) static LOCAL_PROFILE_SAMPLES: Mutex<Vec<LocalProfileSample>> = Mutex::new(Vec::new());
const LOCAL_PROFILE_SAMPLE_BUFFER_LIMIT: usize = 16_384;
const STUDIO_SOCKET_ID: u64 = 0;

fn recv_studio_thread_msg(
    rx: &Receiver<StudioWebSocketThreadMsg>,
    timeout: Duration,
) -> Result<StudioWebSocketThreadMsg, RecvTimeoutError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        return rx.recv_timeout(timeout);
    }

    #[cfg(target_arch = "wasm32")]
    {
        if timeout == Duration::MAX {
            return rx.recv().map_err(|_| RecvTimeoutError::Disconnected);
        }

        let deadline = Cx::time_now() + timeout.as_secs_f64();
        loop {
            match rx.try_recv() {
                Ok(msg) => return Ok(msg),
                Err(TryRecvError::Empty) => {
                    if Cx::time_now() >= deadline {
                        return Err(RecvTimeoutError::Timeout);
                    }
                    std::thread::yield_now();
                }
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
            }
        }
    }
}

fn studio_ws_send_binary(data: Vec<u8>) -> Result<(), ()> {
    let runtime = STUDIO_NET_RUNTIME
        .lock()
        .ok()
        .and_then(|runtime| runtime.as_ref().cloned())
        .ok_or(())?;
    runtime
        .ws_send(LiveId(STUDIO_SOCKET_ID), WsSend::Binary(data))
        .map_err(|_| ())
}

impl Cx {
    pub fn has_studio_web_socket() -> bool {
        HAS_STUDIO_WEB_SOCKET.load(Ordering::SeqCst)
    }

    /// Enable stdout mode for studio messages. When enabled,
    /// `send_studio_message` writes JSON lines to stdout instead of
    /// the websocket. Also sets `HAS_STUDIO_WEB_SOCKET` so that
    /// internal code paths (screenshots, widget dumps, event profiling)
    /// remain active.
    pub fn set_studio_stdout_mode(enabled: bool) {
        STUDIO_STDOUT_MODE.store(enabled, Ordering::SeqCst);
        HAS_STUDIO_WEB_SOCKET.store(enabled, Ordering::SeqCst);
    }

    /// Set a control channel for receiving StudioToApp messages.
    /// Messages are polled by the event loop and dispatched as events.
    /// The sender should call `SignalToUI::set_ui_signal()` after sending.
    pub fn set_control_channel(rx: Receiver<StudioToApp>) {
        *CONTROL_CHANNEL.lock().unwrap() = Some(rx);
    }

    pub fn local_profile_capture_enabled() -> bool {
        LOCAL_PROFILE_CAPTURE_ENABLED.load(Ordering::SeqCst)
    }

    pub fn set_local_profile_capture_enabled(enabled: bool) {
        LOCAL_PROFILE_CAPTURE_ENABLED.store(enabled, Ordering::SeqCst);
        if !enabled {
            if let Ok(mut samples) = LOCAL_PROFILE_SAMPLES.lock() {
                samples.clear();
            }
        }
    }

    pub fn take_local_profile_samples() -> Vec<LocalProfileSample> {
        if let Ok(mut samples) = LOCAL_PROFILE_SAMPLES.lock() {
            return samples.drain(..).collect();
        }
        Vec::new()
    }

    fn capture_local_profile_sample(msg: &AppToStudio) {
        if !Self::local_profile_capture_enabled() {
            return;
        }

        let sample = match msg {
            AppToStudio::EventSample(sample)
                if Event::name_from_u32(sample.event_u32) == "Draw" =>
            {
                LocalProfileSample::Event(sample.clone())
            }
            AppToStudio::GPUSample(sample) => LocalProfileSample::GPU(sample.clone()),
            AppToStudio::GCSample(sample) => LocalProfileSample::GC(sample.clone()),
            _ => return,
        };

        if let Ok(mut samples) = LOCAL_PROFILE_SAMPLES.lock() {
            samples.push(sample);
            if samples.len() > LOCAL_PROFILE_SAMPLE_BUFFER_LIMIT {
                let remove = samples.len() - LOCAL_PROFILE_SAMPLE_BUFFER_LIMIT;
                samples.drain(0..remove);
            }
        }
        SignalToUI::set_ui_signal();
    }

    fn run_studio_websocket_thread(&mut self) {
        let (tx, rx) = channel();
        *STUDIO_WEB_SOCKET_THREAD_SENDER.lock().unwrap() = Some(tx);

        self.spawn_thread(move || {
            let mut app_to_studio = AppToStudioVec(Vec::new());
            let mut first_message_time = None;
            let default_collect_time = Duration::from_millis(16);
            let urgent_collect_time = Duration::from_millis(1);
            let mut collect_time = default_collect_time;
            let mut cycle_time = Duration::MAX;

            loop {
                match recv_studio_thread_msg(&rx, cycle_time) {
                    Ok(StudioWebSocketThreadMsg::AppToStudio { message }) => {
                        if first_message_time.is_none() {
                            first_message_time = Some(Cx::time_now());
                        }
                        if matches!(
                            &message,
                            AppToStudio::BeforeStartup
                                | AppToStudio::AfterStartup
                                | AppToStudio::RequestAnimationFrame
                                | AppToStudio::DrawCompleteAndFlip(_)
                        ) {
                            collect_time = urgent_collect_time;
                        }
                        app_to_studio.0.push(message);
                        cycle_time = collect_time;
                    }
                    Ok(StudioWebSocketThreadMsg::Terminate) => {
                        break;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }

                if let Some(first_time) = first_message_time {
                    if (Cx::time_now() - first_time) >= collect_time.as_secs_f64() {
                        if studio_ws_send_binary(app_to_studio.serialize_bin()).is_err() {
                            println!("Studio websocket disconnected!");
                            break;
                        }
                        app_to_studio.0.clear();
                        first_message_time = None;
                        collect_time = default_collect_time;
                        cycle_time = Duration::MAX;
                    }
                }
            }
            *STUDIO_WEB_SOCKET_THREAD_SENDER.lock().unwrap() = None;
        });
    }

    fn start_studio_websocket(&mut self, studio_http: &str) {
        if studio_http.is_empty() {
            return;
        }
        self.studio_http = studio_http.into();

        #[cfg(all(not(target_os = "tvos"), not(target_os = "ios")))]
        {
            HAS_STUDIO_WEB_SOCKET.store(true, Ordering::SeqCst);
            let mut request = HttpRequest::new(studio_http.to_string(), HttpMethod::GET);
            request.set_websocket_transport(WebSocketTransport::PlainTcp);
            *STUDIO_NET_RUNTIME.lock().unwrap() = Some(self.net.clone());
            if let Err(err) = self.net.ws_open(LiveId(STUDIO_SOCKET_ID), request) {
                crate::error!("could not open studio websocket: {err}");
                HAS_STUDIO_WEB_SOCKET.store(false, Ordering::SeqCst);
                *STUDIO_NET_RUNTIME.lock().unwrap() = None;
            }
        }
    }

    pub fn stop_studio_websocket(&mut self) {
        let _ = self.net.ws_close(LiveId(STUDIO_SOCKET_ID));
        *STUDIO_NET_RUNTIME.lock().unwrap() = None;
        HAS_STUDIO_WEB_SOCKET.store(false, Ordering::SeqCst);
        let sender = STUDIO_WEB_SOCKET_THREAD_SENDER.lock().unwrap();
        if let Some(sender) = &*sender {
            let _ = sender.send(StudioWebSocketThreadMsg::Terminate);
        }
    }

    #[cfg(any(target_os = "tvos", target_os = "ios"))]
    pub fn start_studio_websocket_delayed(&mut self) {
        HAS_STUDIO_WEB_SOCKET.store(true, Ordering::SeqCst);
        let mut request = HttpRequest::new(self.studio_http.clone(), HttpMethod::GET);
        request.set_websocket_transport(WebSocketTransport::PlainTcp);
        *STUDIO_NET_RUNTIME.lock().unwrap() = Some(self.net.clone());
        if let Err(err) = self.net.ws_open(LiveId(STUDIO_SOCKET_ID), request) {
            crate::error!("could not open delayed studio websocket: {err}");
            HAS_STUDIO_WEB_SOCKET.store(false, Ordering::SeqCst);
            *STUDIO_NET_RUNTIME.lock().unwrap() = None;
        }
    }

    pub fn init_websockets(&mut self, studio_http: &str) {
        self.run_studio_websocket_thread();
        self.start_studio_websocket(studio_http);
    }

    #[cfg(not(target_os = "android"))]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn recv_studio_websocket_message(&mut self) -> Option<WebSocketMessage> {
        loop {
            let response = self.net.recv().ok()?;
            match response {
                NetworkResponse::WsOpened { socket_id } if socket_id.0 == STUDIO_SOCKET_ID => {
                    return Some(WebSocketMessage::Opened);
                }
                NetworkResponse::WsClosed { socket_id } if socket_id.0 == STUDIO_SOCKET_ID => {
                    return Some(WebSocketMessage::Closed);
                }
                NetworkResponse::WsError { socket_id, message }
                    if socket_id.0 == STUDIO_SOCKET_ID =>
                {
                    return Some(WebSocketMessage::Error(message));
                }
                NetworkResponse::WsMessage { socket_id, message }
                    if socket_id.0 == STUDIO_SOCKET_ID =>
                {
                    return Some(match message {
                        WsMessage::Binary(data) => WebSocketMessage::Binary(data),
                        WsMessage::Text(data) => WebSocketMessage::String(data),
                    });
                }
                response => {
                    if matches!(
                        response,
                        NetworkResponse::WsOpened { .. }
                            | NetworkResponse::WsClosed { .. }
                            | NetworkResponse::WsError { .. }
                            | NetworkResponse::WsMessage { .. }
                    ) {
                        self.handle_script_web_socket_event(response.clone());
                    }
                    self.handle_script_network_events(std::slice::from_ref(&response));
                    self.call_event_handler(&Event::NetworkResponses(vec![response]));
                }
            }
        }
    }

    pub fn send_studio_message(msg: AppToStudio) {
        Self::capture_local_profile_sample(&msg);
        if STUDIO_STDOUT_MODE.load(Ordering::SeqCst) {
            use std::io::Write;
            let _ = std::io::stdout().write_all(msg.to_json().as_bytes());
            let _ = std::io::stdout().flush();
            return;
        }
        if !Cx::has_studio_web_socket() {
            return;
        }

        if let Some(sender) = STUDIO_WEB_SOCKET_THREAD_SENDER.lock().unwrap().as_ref() {
            let _ = sender.send(StudioWebSocketThreadMsg::AppToStudio { message: msg });
        } else {
            let _ = studio_ws_send_binary(AppToStudioVec(vec![msg]).serialize_bin());
        }
    }
}
