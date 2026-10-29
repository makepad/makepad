//! Native-layer messages share the hosted hub and its ordered descriptor channel.
use super::*;
use makepad_studio_protocol::{NativeAppEvent, NativeAppId, NativeAppRequest};
use std::os::fd::OwnedFd;

pub struct Incoming {
    pub event: NativeAppEvent,
    pub fds: Vec<OwnedFd>,
}
pub struct Outgoing { pub request: NativeAppRequest, pub fence: Option<OwnedFd> }
static INCOMING: Mutex<Vec<Incoming>> = Mutex::new(Vec::new());
static OUTGOING: Mutex<Vec<Outgoing>> = Mutex::new(Vec::new());
static BRIDGE: OnceLock<fn()> = OnceLock::new();
static PRESENTED: Mutex<Vec<(u32, Id, u64)>> = Mutex::new(Vec::new());
static ACTIVE: AtomicBool = AtomicBool::new(false);
pub(crate) fn active(on: bool) { ACTIVE.store(on, Ordering::Release); }

pub fn trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MAKEPAD_TRACE").is_ok_and(|s| s.split(',').any(|s| s == "native-app")))
}

pub fn enable_bridge(wake: fn()) { let _ = BRIDGE.set(wake); }
pub fn bridge_enabled() -> bool { BRIDGE.get().is_some() }
pub fn receive(incoming: Incoming) {
    let session = id(&incoming.event).session;
    lock(&INCOMING).push(incoming);
    crate::hal_wake_session(session);
}
pub fn take_outgoing() -> Vec<Outgoing> { std::mem::take(&mut *lock(&OUTGOING)) }
pub fn presented(session: u32, window: Id, frame: u64) {
    if !ACTIVE.load(Ordering::Acquire) { return; }
    lock(&PRESENTED).push((session, window, frame));
    crate::hal_wake_session(session);
}
pub(crate) fn take_presented() -> Vec<(u32, Id, u64)> { std::mem::take(&mut *lock(&PRESENTED)) }

pub(crate) fn request(request: NativeAppRequest, fence: Option<OwnedFd>) {
    lock(&OUTGOING).push(Outgoing { request, fence });
    if let Some(wake) = BRIDGE.get() { wake(); } else { Shared::global().signal(); }
}
pub(crate) fn take() -> Vec<Incoming> { std::mem::take(&mut *lock(&INCOMING)) }
pub(crate) fn id(event: &NativeAppEvent) -> NativeAppId {
    match event { NativeAppEvent::Buffer { id, .. } | NativeAppEvent::Commit { id, .. } |
        NativeAppEvent::Unmapped { id } | NativeAppEvent::Forget { id, .. } | NativeAppEvent::State { id, .. } => *id }
}

impl App {
    pub(super) fn native_message(&mut self, event: NativeAppEvent) {
        let native = id(&event);
        let mut fds = Vec::new();
        if let NativeAppEvent::Buffer { buffer, desc, .. } = &event {
            self.connect_aux();
            let result = (|| -> Result<(), String> {
                let endpoint = self.aux.as_ref().ok_or("native buffer before descriptor channel")?;
                for _ in &desc.planes {
                    let (got, fd) = endpoint.recv().map_err(|e| e.to_string())?;
                    if got.as_u64() != *buffer { return Err("native buffer descriptor order mismatch".into()); }
                    fds.push(fd);
                }
                Ok(())
            })();
            if let Err(error) = result {
                self.state.shared.push((native.session, Id::new(native.index, native.generation)), |q, id| {
                    q.native_layer_state(id, crate::native_layer::state::CRASHED, 0, 0, error.as_bytes());
                });
                return;
            }
        }
        lock(&INCOMING).push(Incoming { event, fds });
        self.state.shared.push((native.session, Id::NONE), |q, _| q.wake());
    }

    pub(super) fn native_outgoing(&mut self) {
        let pending = take_outgoing();
        for Outgoing { request, fence } in pending {
            if let Some(fence) = fence {
                let NativeAppRequest::Release { buffer, .. } = request else { continue };
                let result = self.aux.as_ref().ok_or_else(|| "missing release descriptor channel".to_string())
                    .and_then(|aux| aux.send((PresentableImageId::from_u64(buffer), fence)).map_err(|e| e.to_string()));
                if let Err(error) = result {
                    eprintln!("[hal-wm] native release: {error}");
                    self.quit();
                    return;
                }
            }
            self.hub.send(vec![AppToStudio::NativeApp(request)]);
        }
    }
}
