//! External renderers use the same pack/encoder as native XR passes. GPU ownership
//! stays with the caller: submit only finished textures, then wait for the local
//! frame's GPU completion before letting their producer write them again.
use super::{sink::XrPack, XrServer};
use makepad_widgets::*;
use makepad_widgets::makepad_draw::xr_sink::{self, XrFrameSink, XrPresented};
use makepad_widgets::makepad_platform::makepad_hal::XrFrame;
use std::{cell::RefCell, rc::Rc, sync::{Arc, atomic::Ordering}, time::Instant};

pub use makepad_desk_link::xr::XrRenderEye;

/// A prediction received from the headset. `sample_ns` and `display_ns` use the
/// headset's clock; `received_ns` uses the host HAL's monotonic clock. Keeping
/// these distinct prevents an external renderer from silently mixing clocks.
#[derive(Clone, Debug)]
pub struct TrackingSnapshot {
    pub seq: u64,
    pub sample_ns: u64,
    pub display_ns: u64,
    pub received_ns: u64,
    pub frame: XrFrame,
    pub(crate) arrived: Instant,
}

/// Optional projection depth. The sampled depth range maps from `near_m` to
/// `far_m`; an infinite far plane is supported. No depth means no invented room
/// occlusion or positional depth reconstruction.
pub struct ExternalDepth<'a> {
    pub texture: &'a Texture,
    pub range: [f32; 2],
    pub near_m: f32,
    pub far_m: f32,
}

pub struct ExternalEye<'a> {
    pub color: &'a Texture,
    pub depth: Option<ExternalDepth<'a>>,
    pub size: [u32; 2],
    pub rendered: XrRenderEye,
}

pub struct ExternalFrame<'a> {
    pub tracking: &'a TrackingSnapshot,
    pub eyes: [ExternalEye<'a>; 2],
    /// The colour is premultiplied with meaningful coverage. False forces opaque
    /// colour; an unused game render target alpha channel is never room coverage.
    pub alpha: bool,
    pub focus_m: f32,
}

#[derive(Clone)]
pub struct ExternalRenderer {
    pack: Rc<RefCell<XrPack>>,
    pose_wire: Rc<RefCell<Vec<u8>>>,
}

#[derive(Default)]
struct ExternalSlot(Option<ExternalRenderer>);

struct Signals(ExternalRenderer);
impl XrFrameSink for Signals {
    fn presented(&mut self, _cx: &mut Cx2d, _frame: &XrPresented) {}
    fn empty(&mut self, _cx: &mut Cx, _why: &'static str) {}
    fn signal(&mut self, cx: &mut Cx) { self.0.pack.borrow_mut().wake(cx); }
}

impl XrServer {
    /// Installs the server for a renderer outside Makepad's XR pass. The caller
    /// handles new tracking on UI signals and submits its finished eye textures.
    /// Native XR-pass users continue using `install` unchanged.
    pub fn install_external(cx: &mut Cx, app: &str) -> Option<(Self, ExternalRenderer)> {
        let server = Self::install(cx, app)?;
        if let Some(renderer) = cx.global::<ExternalSlot>().0.clone() { return Some((server, renderer)); }
        server.shared.external.store(true, Ordering::Release);
        let signal = SignalToUI::new();
        *server.shared.wake.lock().unwrap() = Some(signal.clone());
        let renderer = ExternalRenderer { pack: Rc::new(RefCell::new(XrPack::new(cx, server.clone(), signal))), pose_wire: Default::default() };
        xr_sink::set_xr_frame_sink(cx, Box::new(Signals(renderer.clone())));
        cx.global::<ExternalSlot>().0 = Some(renderer.clone());
        Some((server, renderer))
    }

    /// Latest tracking, without waiting for network I/O. An Arc clone under a
    /// short lock; snapshot decoding and allocation happen on the input worker.
    pub fn tracking(&self) -> Option<Arc<TrackingSnapshot>> {
        if !self.frames_flowing() { return None; }
        self.shared.tracking.lock().unwrap().clone()
    }

    pub fn session_refresh_mhz(&self) -> Option<u32> {
        self.shared.live.lock().unwrap().as_ref().map(|l| l.refresh_mhz)
    }
}

impl ExternalRenderer {
    /// Backpressure, without a UI wait. Keep the pending frame and retry on a
    /// later Signal when this is false; the caller retains its GPU image lease.
    pub fn ready(&self) -> bool { self.pack.borrow().external_ready() }

    /// Schedule the completed game frame on the remote headset's render clock,
    /// independently of the desktop refresh. On the resulting XrUpdate event,
    /// request the app's draw; present its external textures during that draw.
    /// False means the XR window is still being created; retry on its next event.
    pub fn request_draw(&self, cx: &mut Cx, tracking: &TrackingSnapshot) -> bool {
        use makepad_widgets::makepad_platform::makepad_hal::{buffers::Wire, xr_remote};
        cx.xr_start_presenting();
        let mut wire = self.pose_wire.borrow_mut();
        wire.clear();
        tracking.frame.write(&mut wire);
        xr_remote::hal_xr_remote_pose(tracking.seq, tracking.display_ns, &wire)
    }

    /// True means packing was recorded on the real GPU. A false return leaves
    /// the frame with the caller for retry; invalid metadata returns an error.
    pub fn present(&self, cx: &mut Cx2d, frame: &ExternalFrame<'_>) -> Result<bool, String> {
        self.pack.borrow_mut().pack_external(cx, frame)
    }
}
