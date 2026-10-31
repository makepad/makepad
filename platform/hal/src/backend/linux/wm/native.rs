//! Native-layer messages share the hosted hub and its ordered descriptor channel.
use super::*;
use makepad_studio_protocol::{NativeAppEvent, NativeAppId, NativeAppRequest};
use std::os::fd::OwnedFd;
use std::sync::atomic::AtomicU32;

pub struct Incoming {
    pub event: NativeAppEvent,
    pub fds: Vec<OwnedFd>,
}
pub struct Outgoing { pub request: NativeAppRequest, pub fence: Option<OwnedFd> }
static INCOMING: Mutex<Vec<Incoming>> = Mutex::new(Vec::new());
static OUTGOING: Mutex<Vec<Outgoing>> = Mutex::new(Vec::new());
/// The session of a WM-hosted Cx that relays native-layer messages itself (`HostedNativeBridge`):
/// its requests go out as `HostedNativeOut` events, its incoming ones come as
/// `HostedNativeIn` commands. `u32::MAX`: none.
static BRIDGE: AtomicU32 = AtomicU32::new(u32::MAX);
static PRESENTED: Mutex<Vec<(u32, Id, u64)>> = Mutex::new(Vec::new());
static ACTIVE: AtomicBool = AtomicBool::new(false);
pub(crate) fn active(on: bool) { ACTIVE.store(on, Ordering::Release); }

#[makepad_relax_api::relax_api]
pub fn trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MAKEPAD_TRACE").is_ok_and(|s| s.split(',').any(|s| s == "native-app")))
}

pub(crate) fn enable_bridge(session: u32) { BRIDGE.store(session, Ordering::Release); }
pub fn bridge_enabled() -> bool { BRIDGE.load(Ordering::Acquire) != u32::MAX }
pub(crate) fn receive(incoming: Incoming) {
    let session = id(&incoming.event).session;
    lock(&INCOMING).push(incoming);
    crate::hal_wake_session(session);
}
pub(crate) fn take_outgoing() -> Vec<Outgoing> { std::mem::take(&mut *lock(&OUTGOING)) }

/// A `HostedNativeIn` command's payload: the descriptor count, the descriptors (raw fds the
/// sender gave up), then the event (SerBin). The descriptors are owned here from the start, so
/// a payload that does not decode still closes them.
pub(crate) fn receive_bytes(bytes: &[u8]) {
    use makepad_micro_serde::DeBin;
    use std::os::fd::FromRawFd;
    let word = |k: usize| bytes.get(k * 4..k * 4 + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let Some(n) = word(0).filter(|n| *n >= 0) else { return };
    let mut fds = Vec::with_capacity(n as usize);
    for k in 0..n as usize {
        // SAFETY: the sender handed over each descriptor with into_raw_fd (taken once)
        if let Some(fd) = word(1 + k).filter(|fd| *fd >= 0) { fds.push(unsafe { OwnedFd::from_raw_fd(fd) }); }
    }
    let Some(rest) = bytes.get((1 + n as usize) * 4..) else { return };
    match NativeAppEvent::deserialize_bin(rest) {
        Ok(event) => receive(Incoming { event, fds }),
        Err(error) => eprintln!("[hal] hosted native message: {error:?}"),
    }
}

/// The bridge session's outgoing requests as its events (each: the fence fd or -1, then the
/// request, SerBin; the receiver owns the fence).
pub(crate) fn drain_events(sid: u32, world: &mut crate::session::World) {
    use makepad_micro_serde::SerBin;
    use std::os::fd::IntoRawFd;
    if BRIDGE.load(Ordering::Acquire) != sid { return; }
    for Outgoing { request, fence } in take_outgoing() {
        let mut bytes = fence.map_or(-1, |f| f.into_raw_fd()).to_le_bytes().to_vec();
        bytes.extend(request.serialize_bin());
        world.to_session(sid, |q| q.hosted_native_out(&bytes));
    }
}
pub(crate) fn presented(session: u32, window: Id, frame: u64) {
    if !ACTIVE.load(Ordering::Acquire) { return; }
    lock(&PRESENTED).push((session, window, frame));
    crate::hal_wake_session(session);
}
pub(crate) fn take_presented() -> Vec<(u32, Id, u64)> { std::mem::take(&mut *lock(&PRESENTED)) }

pub(crate) fn request(request: NativeAppRequest, fence: Option<OwnedFd>) {
    lock(&OUTGOING).push(Outgoing { request, fence });
    match BRIDGE.load(Ordering::Acquire) {
        u32::MAX => Shared::global().signal(),
        session => crate::hal_wake_session(session),
    }
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
