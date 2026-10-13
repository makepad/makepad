//! Process hosting over the existing Studio protocol. Only handles and texture ids cross
//! this client; importing a native surface and rendering into it belong to the HAL backend.
//! Host ticks use the ordinary HAL frame builder, and GPU completion returns the image to
//! the host. This is also the window path for future fd-based shared-image transports.

use crate::{
    event::{hal_intake::hal_window_id, Event, TweakRayEvent},
    makepad_math::*,
    os::shared_framebuf::PollTimers,
    texture::{Texture, TextureFormat},
    window::{CxWindowPool, WindowId},
    Cx,
};
use makepad_hal::{texture_handle, window_kind, window_state, Id};
use crate::studio::{
    AppToStudio, PresentableDraw, PresentableImageId, SharedSwapchain,
    StudioToApp,
};
use std::collections::HashMap;
#[cfg(not(target_os = "linux"))]
use crate::studio::SharedPresentableImage;

#[derive(Default)]
pub(crate) struct HostedClient {
    windows: HashMap<Id, HostedSwapchain>,
    pending: HashMap<u64, PendingPresent>,
    /// Logical timers run only on host Ticks; the host owns their wake schedule.
    pub(super) timers: PollTimers,
    pub(super) demand_changed: bool,
    /// Geometry the host sent for windows this app had not made yet.
    early_geom: EarlyGeom,
    #[cfg(target_os = "linux")]
    inbox: Option<crate::os::hosted_gpu::GpuInbox>,
}

/// A host geometry message: `[left, top, width, height]` and the dpi factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HostGeom {
    pub rect: [f64; 4],
    pub dpi: f64,
}

/// The host may size a window before the app has made it (it learns the
/// window id from the app's own channel, and a slow host's first beat can
/// overtake the app's creation of it). Such a size is kept, the newest per
/// window, and applied when the window is made: dropping it left the window
/// unsized until the next resize, which for a hosted page never comes.
#[derive(Default)]
pub(crate) struct EarlyGeom {
    sizes: Vec<(usize, HostGeom)>,
}

impl EarlyGeom {
    pub fn keep(&mut self, window_id: usize, geom: HostGeom) {
        self.sizes.retain(|(id, _)| *id != window_id);
        self.sizes.push((window_id, geom));
    }

    pub fn take(&mut self, window_id: usize) -> Option<HostGeom> {
        let at = self.sizes.iter().position(|(id, _)| *id == window_id)?;
        Some(self.sizes.remove(at).1)
    }
}

struct HostedSwapchain {
    width: u32,
    height: u32,
    images: Vec<HostedImage>,
    next: usize,
}

#[derive(PartialEq)]
enum ImportState {
    Pending,
    Ready,
    Retry,
}

struct HostedImage {
    image: PresentableImageId,
    handle: SharedHandle,
    texture: Texture,
    hal: Id,
    import: ImportState,
    in_flight: Option<u64>,
    warned: bool,
}

struct SharedHandle {
    value: u64,
    #[cfg(target_os = "linux")]
    session: u32,
}
#[cfg(target_os = "linux")]
impl Drop for SharedHandle {
    fn drop(&mut self) { makepad_hal::Hal::global().dmabuf_release(self.session, self.value); }
}

struct PendingPresent {
    window: WindowId,
    draw: PresentableDraw,
    // A replaced swapchain stays alive until the GPU finishes its already-submitted frames.
    _texture: Texture,
    #[cfg(target_os = "linux")]
    hal: Id,
    #[cfg(target_os = "linux")]
    session: u32,
}

impl HostedClient {
    pub(crate) fn target(&self, window: WindowId, width: usize, height: usize) -> Option<(usize, Id)> {
        let swapchain = self.windows.get(&hal_window_id(window))?;
        if width > swapchain.width as usize || height > swapchain.height as usize {
            // Geometry can precede the matching swapchain during resize. Keep the pass dirty.
            return None;
        }
        (0..swapchain.images.len()).find_map(|offset| {
            let index = (swapchain.next + offset) % swapchain.images.len();
            let image = &swapchain.images[index];
            let ready = image.import == ImportState::Ready && image.in_flight.is_none();
            #[cfg(target_os = "linux")]
            let ready = ready && makepad_hal::Hal::global().dmabuf_write_available(image.handle.session, image.hal).unwrap_or(false);
            ready.then_some((index, image.hal))
        })
    }

    pub(crate) fn submitted(&mut self, window: WindowId, index: usize, frame: u64, size: [u32; 2]) {
        let swapchain = self.windows.get_mut(&hal_window_id(window)).expect("selected hosted swapchain");
        let image = &mut swapchain.images[index];
        image.in_flight = Some(frame);
        self.pending.insert(frame, PendingPresent {
            window,
            draw: PresentableDraw { window_id: window.id(), target_id: image.image, width: size[0], height: size[1], sequence: 0 },
            _texture: image.texture.clone(),
            #[cfg(target_os = "linux")]
            hal: image.hal,
            #[cfg(target_os = "linux")]
            session: image.handle.session,
        });
        swapchain.next = (index + 1) % swapchain.images.len();
    }

    pub(crate) fn completed(&mut self, window: Id, frame: u64) -> Option<PresentableDraw> {
        if !self.pending.get(&frame).is_some_and(|p| hal_window_id(p.window) == window) {
            return None;
        }
        let pending = self.pending.remove(&frame)?;
        if let Some(swapchain) = self.windows.get_mut(&window) {
            for image in &mut swapchain.images {
                if image.image == pending.draw.target_id && image.in_flight == Some(frame) {
                    image.in_flight = None;
                }
            }
        }
        #[cfg(target_os = "linux")]
        let pending = {
            let mut pending = pending;
            match makepad_hal::Hal::global().dmabuf_sequence(pending.session, pending.hal) {
                Ok(sequence) => pending.draw.sequence = sequence,
                Err(error) => { crate::error!("hal hosted completion: {error}"); return None; }
            }
            pending
        };
        Some(pending.draw)
    }

    pub(crate) fn window_closed(&mut self, window: WindowId) {
        self.windows.remove(&hal_window_id(window));
        self.pending.retain(|_, pending| pending.window != window);
    }

    pub(crate) fn imported(&mut self, texture: Id, ok: bool) {
        for swapchain in self.windows.values_mut() {
            for image in &mut swapchain.images {
                if image.hal != texture { continue; }
                image.import = if ok { ImportState::Ready } else { ImportState::Retry };
                if !ok && !std::mem::replace(&mut image.warned, true) {
                    crate::warning!("hal: shared image {:?} could not be imported; retrying on host ticks", image.image);
                }
            }
        }
    }
}

// Only decoding/transport is target-specific; swapchain installation, rendering,
// damage, retries and completion use the same client path on every backend.
#[cfg(not(target_os = "linux"))]
fn image_handle(image: &SharedPresentableImage) -> Result<SharedHandle, &'static str> {
    #[cfg(target_os = "macos")]
    { Ok(SharedHandle { value: image.iosurface_id as u64 }) }
    // Windows: the D3D11 backend's wire handle, naming the host's NT shared texture
    #[cfg(target_os = "windows")]
    { Ok(SharedHandle { value: image.handle }) }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    { let _ = image; Err("the HAL backend has no hosted shared-image transport on this platform yet") }
}

impl Cx {
    pub(crate) fn hal_hosted_create_window(&mut self, window: WindowId) {
        self.windows[window].is_created = true;
        self.hal.windows.insert(window.0, super::HalWindow::default());
        self.hal.frames.w.window_create(hal_window_id(window), window_kind::HOSTED, Id::NONE, [0.0; 4], false);
        Self::send_studio_message(AppToStudio::CreateWindow {
            window_id: window.id(), kind_id: self.windows[window].kind_id,
        });
        let early = self.hal.hosted.as_mut().and_then(|hosted| hosted.early_geom.take(window.id()));
        if let Some(geom) = early {
            self.hal_hosted_geom(window, geom);
        }
    }

    /// The host's geometry for a window this app has made.
    fn hal_hosted_geom(&mut self, window: WindowId, geom: HostGeom) {
        let [left, top, width, height] = geom.rect;
        self.hal.frames.w.window_hosted_geom(hal_window_id(window), [left as f32, top as f32, width as f32, height as f32], geom.dpi as f32);
        // Apply before the following pointer message in this batch; the HAL has no
        // separate OS geometry event to wait for and must not echo an older scale.
        self.hal_window_geom(window, dvec2(left, top), dvec2(width, height), geom.dpi, window_state::RESTORE);
    }

    fn hal_hosted_swapchain(&mut self, swapchain: SharedSwapchain) {
        #[cfg(target_os = "linux")]
        {
            if let Err(error) = self.hal.hosted.as_mut().unwrap().inbox.as_mut().unwrap()
                .enqueue(crate::os::hosted_gpu::SwapchainRequest { swapchain }) {
                crate::error!("hal hosted descriptors: {error}");
            }
        }
        #[cfg(not(target_os = "linux"))]
        match swapchain.presentable_images.iter().map(image_handle).collect() {
            Ok(handles) => self.hal_hosted_install(swapchain, handles),
            Err(reason) => self.hal.frames.gap(reason),
        }
    }

    fn hal_hosted_install(&mut self, swapchain: SharedSwapchain, handles: Vec<SharedHandle>) {
        let Some(window) = self.windows.id_iter().find(|w| w.id() == swapchain.window_id && self.windows[*w].is_created) else { return };
        let hosted = self.hal.hosted.as_ref().expect("hosted client");
        if hosted.windows.get(&hal_window_id(window)).is_some_and(|old| {
            old.width == swapchain.alloc_width && old.height == swapchain.alloc_height
                && old.images.len() == swapchain.presentable_images.len()
                && old.images.iter().zip(swapchain.presentable_images.iter().zip(&handles))
                    .all(|(old, (image, handle))| old.image == image.id && {
                        #[cfg(target_os = "linux")]
                        { let _ = handle; true } // PresentableImageId identifies the allocation; tickets are per delivery.
                        #[cfg(not(target_os = "linux"))]
                        { old.handle.value == handle.value }
                    })
        }) {
            return;
        }
        let images = swapchain.presentable_images.iter().zip(handles).map(|(image, handle)| {
            let texture = Texture::new_with_format(self, TextureFormat::SharedBGRAu8 {
                id: image.id, width: swapchain.alloc_width as usize, height: swapchain.alloc_height as usize, initial: true,
            });
            let id = texture.texture_id();
            self.textures[id].alloc_shared();
            let hal = self.hal.frames.external_texture(id.0, id.generation());
            self.hal.frames.w.texture_import(hal, handle.value, texture_handle::NATIVE);
            HostedImage { image: image.id, handle, texture, hal, import: ImportState::Pending, in_flight: None, warned: false }
        }).collect();
        self.hal.hosted.as_mut().unwrap().windows.insert(hal_window_id(window), HostedSwapchain {
            width: swapchain.alloc_width, height: swapchain.alloc_height, images, next: 0,
        });
        self.redraw_all();
    }

    fn hal_hosted_retry_imports(&mut self) {
        let hosted = self.hal.hosted.as_mut().expect("hosted client");
        for swapchain in hosted.windows.values_mut() {
            for image in &mut swapchain.images {
                if image.import == ImportState::Retry {
                    self.hal.frames.w.texture_import(image.hal, image.handle.value, texture_handle::NATIVE);
                    image.import = ImportState::Pending;
                }
            }
        }
    }

    /// Nonblocking: the HAL pump handles GPU completions and network wakeups even
    /// when the host is idle. The shared network drain parks this socket's messages for us.
    /// None ends the session; the bool says a batch was consumed (drain again before sleeping).
    pub(super) fn hal_hosted_poll(&mut self) -> Option<bool> {
        #[cfg(target_os = "linux")]
        self.hal_hosted_descriptors();
        let mut batch = Vec::new();
        let closed = self.stdin_drain_host_batches(&mut batch);
        let received = !batch.is_empty();
        if received { crate::memory_watchdog::note_stdin_host_message(); }
        Self::stdin_coalesce_host_batch(&mut batch);
        for msg in batch {
            if self.hal_hosted_message(msg) { return None; }
        }
        self.handle_actions();
        if closed {
            self.call_event_handler(&Event::Shutdown);
            return None;
        }
        Some(received)
    }

    fn hal_hosted_message(&mut self, msg: StudioToApp) -> bool {
        match msg {
            StudioToApp::Swapchain(swapchain) => self.hal_hosted_swapchain(swapchain),
            // The shared-image protocol presents through DrawCompleteAndFlip.
            StudioToApp::RunViewFrameRequest(_) => {},
            StudioToApp::WindowGeomChange { window_id, dpi_factor, left, top, width, height } => {
                if ![left, top, width, height, dpi_factor].iter().all(|v| v.is_finite())
                    || dpi_factor <= 0.0 || width < 0.0 || height < 0.0 { return false; }
                let geom = HostGeom { rect: [left, top, width, height], dpi: dpi_factor };
                let made = self.windows.id_iter().find(|w| w.id() == window_id && self.windows[*w].is_created);
                match made {
                    Some(window) => self.hal_hosted_geom(window, geom),
                    // Not made yet: applied when it is (`hal_hosted_create_window`).
                    None => self.hal.hosted.as_mut().unwrap().early_geom.keep(window_id, geom),
                }
            }
            StudioToApp::Tick => {
                self.stdin_tick_began();
                if !self.hal_handle_platform_ops() {
                    self.call_event_handler(&Event::Shutdown);
                    return true;
                }
                if !self.hosted_hidden {
                    self.hal_hosted_retry_imports();
                }
                if !self.hal_beat() || !self.hal_handle_platform_ops() {
                    self.call_event_handler(&Event::Shutdown);
                    return true;
                }
                self.hal_submit_batch();
                self.hal.hosted.as_mut().unwrap().demand_changed = true;
                self.hal_hosted_announce_demand();
                Self::send_studio_message(AppToStudio::TickDone);
            }
            // Shared dispatch preserves the host's event times, dpi override, captures,
            // cancellation, clipboard replies, keyboard, game input and custom messages.
            other => {
                let pointer = match &other {
                    StudioToApp::MouseDown(e) => Some((dvec2(e.x, e.y), false)),
                    StudioToApp::MouseMove(e) => Some((dvec2(e.x, e.y), true)),
                    StudioToApp::MouseUp(e) | StudioToApp::MouseCancel(e) => Some((dvec2(e.x, e.y), true)),
                    StudioToApp::Scroll(e) => Some((dvec2(e.x, e.y), false)),
                    StudioToApp::Pinch(e) => Some((dvec2(e.x, e.y), false)),
                    StudioToApp::TweakRay(e) => Some((dvec2(e.x, e.y), false)),
                    _ => None,
                };
                let (window, pos) = if let Some((abs, captured)) = pointer {
                    if self.windows.len() == 0 { return false; }
                    match self.fingers.first_mouse_button.filter(|(_, w)| captured && self.windows.is_valid(*w)) {
                        Some((_, window)) => (window, self.windows[window].window_geom.position),
                        None => self.windows.window_id_contains(abs),
                    }
                } else {
                    (self.hal.focused.or_else(|| self.windows.id_iter().next()).unwrap_or(CxWindowPool::id_zero()), dvec2(0.0, 0.0))
                };
                if pointer.is_some() {
                    if self.hal.input_ns == 0 { self.hal.input_ns = makepad_hal::clock_ns(); }
                    if matches!(&other, StudioToApp::MouseDown(_)) { self.hal.focused = Some(window); }
                }
                if let StudioToApp::TweakRay(e) = other {
                    self.call_event_handler(&Event::TweakRay(TweakRayEvent {
                        abs: self.stdin_pointer_abs(dvec2(e.x, e.y), pos, window), window_id: window,
                        modifiers: e.modifiers.into_key_modifiers(), time: e.time,
                        dpi_factor: self.windows[window].window_geom.dpi_factor.max(1.0),
                        hit_widget_uids: Default::default(), hit_rect: Default::default(),
                    }));
                } else {
                    return self.dispatch_studio_msg(other, window, pos);
                }
            }
        }
        false
    }

    pub(super) fn hal_hosted_announce_demand(&mut self) {
        let Some(hosted) = self.hal.hosted.as_mut() else { return };
        let changed = std::mem::take(&mut hosted.demand_changed);
        if changed {
            self.stdin_announce_demand(&self.hal.hosted.as_ref().unwrap().timers);
        }
        // HAL completions can dirty passes without a new Draw/NextFrame request. They
        // share the same coalesced demand as the end of a Tick and worker signals.
        if self.hal_has_work() {
            self.stdin_request_frame();
        }
    }
}

#[cfg(target_os = "linux")]
impl Cx {
    pub(super) fn hal_hosted_start_transport(&mut self) -> Result<(), String> {
        // Connect without waiting for Swapchain: the parent sends that metadata only after
        // it has written the FD batch. Each poll tries once until the parent listens.
        let inbox = crate::os::hosted_gpu::GpuInbox::start(&self.thread_spawner())?;
        self.hal.hosted.as_mut().unwrap().inbox = Some(inbox);
        Ok(())
    }

    fn hal_hosted_descriptors(&mut self) {
        let Some(inbox) = self.hal.hosted.as_mut().and_then(|h| h.inbox.as_mut()) else { return };
        let loaded = match inbox.poll() {
            Ok(loaded) => loaded,
            Err(error) => { crate::error!("hal hosted descriptors: {error}"); return; }
        };
        let session = self.hal.frames.w.session();
        for batch in loaded {
            let swapchain = batch.request.swapchain;
            let handles: Result<Vec<_>, String> = batch.images.into_iter().map(|image| {
                let descriptor = super::linux::hosted_descriptor(image.image, swapchain.alloc_width, swapchain.alloc_height)?;
                let value = unsafe { makepad_hal::Hal::global().dmabuf_register(session, descriptor) }?;
                Ok(SharedHandle { value, session })
            }).collect();
            match handles {
                Ok(handles) => self.hal_hosted_install(swapchain, handles),
                Err(error) => crate::error!("hal hosted import descriptor: {error}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EarlyGeom, HostGeom};

    #[test]
    fn a_size_sent_before_the_window_exists_is_applied_once_it_does() {
        let mut early = EarlyGeom::default();
        let first = HostGeom { rect: [0.0, 0.0, 800.0, 600.0], dpi: 1.0 };
        let newest = HostGeom { rect: [0.0, 0.0, 1280.0, 724.0], dpi: 2.0 };
        early.keep(3, first);
        early.keep(3, newest);
        early.keep(4, first);
        // window 3 is made: it gets the newest size the host sent, once
        assert_eq!(early.take(3), Some(newest));
        assert_eq!(early.take(3), None);
        assert_eq!(early.take(4), Some(first));
    }
}
