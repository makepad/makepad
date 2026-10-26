//! Immersive Quest builds (the APK carries `libopenxr_loader.so`): the OpenXR runtime creates
//! the process's Vulkan instance and device (XR_KHR_vulkan_enable2, through
//! [`VkCreator`]), the host window is a quad layer in front of the person, and frames are paced
//! by xrWaitFrame instead of the Choreographer.
//!
//! The host window's finished frames reach this module as a [`FramePresenter`] (the core's
//! scanout copy, after its fence). The XR thread copies the newest one into the panel's XR
//! swapchain and submits the layer every display frame; between app frames the runtime keeps
//! showing the last released image. Controller aim rays become mouse events on the panel.
use super::{jni, shell::{lock, Shared}, xr_sys::{self as sys, Posef, Quat, Vec3, Xr, XrSpace, XrSwapchain, XrSystemId, Ty}};
use crate::backend::vulkan::surface::{FramePresenter, SurfaceState, VkCreator, VkSurfaceSource};
use crate::backend::vulkan::{Device, Image};
use crate::{clock_ns, lifecycle, session::HOST, Id};
use makepad_vulkan::{commands::PfnGetInstanceProcAddr, vk};
use std::ffi::{c_void, CStr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The panel: its pixels, their density (window points = pixels / density), its size in
/// metres and its distance in front of where the head was when the session began.
const PANEL_PX: [u32; 2] = [1920, 1200];
const PANEL_DPI: f32 = 1.5;
const PANEL_M: [f32; 2] = [2.0, 1.25];
const PANEL_Z: f32 = -2.0;
/// The pointer dot drawn where the aim ray meets the panel (metres; pixels of its image).
const CURSOR_M: f32 = 0.012;
const CURSOR_PX: u32 = 16;
/// Thumbstick travel below this is rest; above it the panel scrolls this many points per frame
/// at full travel.
const STICK_DEAD: f32 = 0.2;
const STICK_SCROLL: f32 = 14.;

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// An XR session shows the host window (the Choreographer and the Activity's surface and
/// window focus are not used).
pub(super) fn active() -> bool { ACTIVE.load(Ordering::Acquire) }

macro_rules! xr_log { ($($t:tt)*) => { eprintln!("[hal-xr] {}", format_args!($($t)*)) } }

struct Runtime { xr: Xr, system: XrSystemId }

impl VkCreator for Runtime {
    unsafe fn create_instance(&self, gipa: PfnGetInstanceProcAddr, info: &vk::InstanceCreateInfo) -> Result<vk::Instance, String> {
        let ci = sys::VulkanInstanceCreateInfoKHR { ty: Ty::VULKAN_INSTANCE_CREATE_INFO_KHR, next: std::ptr::null(), system_id: self.system, create_flags: 0,
            pfn_get_instance_proc_addr: gipa as *const c_void, vulkan_create_info: info as *const _ as *const c_void, vulkan_allocator: std::ptr::null() };
        let (mut handle, mut result) = (0usize, 0i32);
        sys::check((self.xr.xrCreateVulkanInstanceKHR)(self.xr.instance, &ci, &mut handle, &mut result), "xrCreateVulkanInstanceKHR")?;
        if result != 0 { return Err(format!("vkCreateInstance (through OpenXR): {:?}", vk::Result::from_raw(result))); }
        Ok(vk::Instance(handle))
    }
    unsafe fn physical_device(&self, instance: vk::Instance) -> Result<vk::PhysicalDevice, String> {
        let info = sys::VulkanGraphicsDeviceGetInfoKHR { ty: Ty::VULKAN_GRAPHICS_DEVICE_GET_INFO_KHR, next: std::ptr::null(), system_id: self.system, vulkan_instance: instance.0 };
        let mut physical = 0usize;
        sys::check((self.xr.xrGetVulkanGraphicsDevice2KHR)(self.xr.instance, &info, &mut physical), "xrGetVulkanGraphicsDevice2KHR")?;
        Ok(vk::PhysicalDevice(physical))
    }
    unsafe fn create_device(&self, gipa: PfnGetInstanceProcAddr, physical: vk::PhysicalDevice, info: &vk::DeviceCreateInfo) -> Result<vk::Device, vk::Result> {
        let ci = sys::VulkanDeviceCreateInfoKHR { ty: Ty::VULKAN_DEVICE_CREATE_INFO_KHR, next: std::ptr::null(), system_id: self.system, create_flags: 0,
            pfn_get_instance_proc_addr: gipa as *const c_void, vulkan_physical_device: physical.0, vulkan_create_info: info as *const _ as *const c_void, vulkan_allocator: std::ptr::null() };
        let (mut handle, mut result) = (0usize, 0i32);
        let r = (self.xr.xrCreateVulkanDeviceKHR)(self.xr.instance, &ci, &mut handle, &mut result);
        if r < 0 { xr_log!("xrCreateVulkanDeviceKHR: XrResult {r}"); return Err(vk::Result::ERROR_INITIALIZATION_FAILED); }
        if result != 0 { return Err(vk::Result::from_raw(result)); }
        Ok(vk::Device(handle))
    }
}

/// The newest finished host-window frame, waiting for the XR thread, and what to call when it
/// is shown.
type Pending = (Arc<Image>, Box<dyn FnOnce(u64) + Send>);
#[derive(Default)]
struct Frames { next: Mutex<Option<Pending>> }

struct Presenter(Arc<Frames>);
impl FramePresenter for Presenter {
    fn submit(&self, _: u32, _: Id, image: Arc<Image>, presented: Box<dyn FnOnce(u64) + Send>) {
        let replaced = lock(&self.0.next).replace((image, presented));
        // a frame the XR thread never copied: it is over all the same (the app's frames in
        // flight are counted down)
        if let Some((_, done)) = replaced { done(clock_ns()); }
    }
}

pub struct XrSurfaces { rt: Arc<Runtime>, frames: Arc<Frames>, presenter: Arc<dyn FramePresenter>, started: bool }

impl XrSurfaces {
    /// The OpenXR instance and system (before any Vulkan object: the runtime makes those).
    pub fn new(gipa: unsafe extern "system" fn(u64, *const std::ffi::c_char, *mut Option<unsafe extern "system" fn()>) -> i32) -> Result<Self, String> {
        let (vm, activity) = jni::vm_and_activity().ok_or("OpenXR needs the Activity, which is not created yet")?;
        let (xr, enabled) = unsafe { sys::create_instance(gipa, vm, activity, &[
            (c"XR_KHR_android_create_instance", true), (c"XR_KHR_vulkan_enable2", true), (c"XR_KHR_android_thread_settings", false)]) }?;
        let rt = (|| {
            let info = sys::SystemGetInfo { ty: Ty::SYSTEM_GET_INFO, next: std::ptr::null(), form_factor: sys::FORM_FACTOR_HEAD_MOUNTED_DISPLAY };
            let mut system = 0;
            sys::check(unsafe { (xr.xrGetSystem)(xr.instance, &info, &mut system) }, "xrGetSystem")?;
            // required before the runtime makes a Vulkan device
            let mut req = sys::GraphicsRequirementsVulkanKHR { ty: Ty::GRAPHICS_REQUIREMENTS_VULKAN_KHR, next: std::ptr::null_mut(), min_api_version_supported: 0, max_api_version_supported: 0 };
            sys::check(unsafe { (xr.xrGetVulkanGraphicsRequirements2KHR)(xr.instance, system, &mut req) }, "xrGetVulkanGraphicsRequirements2KHR")?;
            let v = |v: u64| format!("{}.{}", v >> 48, (v >> 32) & 0xffff);
            xr_log!("instance {:#x} system {system:#x} extensions {enabled:?}; Vulkan {}..{}", xr.instance, v(req.min_api_version_supported), v(req.max_api_version_supported));
            Ok::<_, String>(system)
        })();
        let system = match rt {
            Ok(system) => system,
            Err(e) => { unsafe { (xr.xrDestroyInstance)(xr.instance); } return Err(e); }
        };
        ACTIVE.store(true, Ordering::Release);
        lock(&Shared::global().state).xr = Some((PANEL_PX, PANEL_DPI));
        Shared::global().display();
        let frames = Arc::new(Frames::default());
        Ok(Self { rt: Arc::new(Runtime { xr, system }), presenter: Arc::new(Presenter(frames.clone())), frames, started: false })
    }
}

impl VkSurfaceSource for XrSurfaces {
    fn instance_extensions(&self) -> Vec<&'static CStr> { Vec::new() }
    fn creator(&self) -> Option<Arc<dyn VkCreator>> { Some(self.rt.clone()) }
    fn device_ready(&mut self, dev: Arc<Device>) -> Result<(), String> {
        if self.started {
            // TODO(QUESTXR): a device made after a loss needs a new session on it
            xr_log!("a replacement Vulkan device: the XR session stays on the lost one");
            return Ok(());
        }
        let session = Session::new(self.rt.clone(), dev, self.frames.clone())?;
        std::thread::Builder::new().name("hal-xr".into()).spawn(move || session.run()).map_err(|e| format!("XR thread: {e}"))?;
        self.started = true;
        Ok(())
    }
    fn presenter(&self, session: u32, window: Id) -> Option<Arc<dyn FramePresenter>> {
        (session == HOST && lock(&Shared::global().registry).host == Some(window)).then(|| self.presenter.clone())
    }
    fn state(&mut self, session: u32, window: Id, _: [u32; 2]) -> SurfaceState {
        if session == HOST && lock(&Shared::global().registry).host == Some(window) { SurfaceState::Managed } else { SurfaceState::Lost }
    }
}

/// An XR swapchain and its Vulkan images.
struct Chain { handle: XrSwapchain, images: Vec<vk::Image> }

/// The copy of a frame into one panel image: its command buffer and fence, and the frame image
/// read until that fence.
struct Slot { pool: vk::CommandPool, cb: vk::CommandBuffer, fence: vk::Fence, keep: Option<Arc<Image>> }

struct Hand { path: u64, space: XrSpace }
struct Input {
    set: u64,
    select: u64,
    scroll: u64,
    hands: [Hand; 2],
    /// the hand whose trigger was pressed last (the right one at first)
    active: usize,
    pressed: bool,
    /// where the pointer is on the panel (window points) and whether it is over it
    at: Option<[f64; 2]>,
    inside: bool,
}

struct Session {
    rt: Arc<Runtime>,
    dev: Arc<Device>,
    frames: Arc<Frames>,
    handle: u64,
    local: XrSpace,
    panel: Chain,
    cursor: Chain,
    slots: Vec<Slot>,
    /// the panel's image formats differ in channel order from the frames: blit, not copy
    blit: bool,
    input: Input,
    running: bool,
    focused: bool,
    /// a frame was released into the panel (its layer may be shown), and the size it drew
    shown: Option<[u32; 2]>,
    cursor_ready: bool,
}

impl Drop for Session {
    fn drop(&mut self) {
        for slot in self.slots.drain(..) {
            unsafe {
                let _ = self.dev.raw.wait_for_fences(&[slot.fence], true, u64::MAX);
                self.dev.raw.destroy_fence(slot.fence, None);
                self.dev.raw.destroy_command_pool(slot.pool, None);
            }
        }
    }
}

fn xr_err(r: i32, what: &str) -> Result<(), String> { sys::check(r, what) }

impl Session {
    fn new(rt: Arc<Runtime>, dev: Arc<Device>, frames: Arc<Frames>) -> Result<Self, String> {
        let xr = &rt.xr;
        let binding = sys::GraphicsBindingVulkanKHR { ty: Ty::GRAPHICS_BINDING_VULKAN_KHR, next: std::ptr::null(), instance: dev.instance.handle().0,
            physical_device: dev.physical.0, device: dev.raw.handle().0, queue_family_index: dev.family, queue_index: 0 };
        let info = sys::SessionCreateInfo { ty: Ty::SESSION_CREATE_INFO, next: &binding as *const _ as *const c_void, create_flags: 0, system_id: rt.system };
        let mut handle = 0;
        xr_err(unsafe { (xr.xrCreateSession)(xr.instance, &info, &mut handle) }, "xrCreateSession")?;
        let space = sys::ReferenceSpaceCreateInfo { ty: Ty::REFERENCE_SPACE_CREATE_INFO, next: std::ptr::null(), reference_space_type: sys::REFERENCE_SPACE_LOCAL, pose_in_reference_space: Posef::IDENTITY };
        let mut local = 0;
        xr_err(unsafe { (xr.xrCreateReferenceSpace)(handle, &space, &mut local) }, "xrCreateReferenceSpace")?;
        let mut count = 0;
        xr_err(unsafe { (xr.xrEnumerateSwapchainFormats)(handle, 0, &mut count, std::ptr::null_mut()) }, "xrEnumerateSwapchainFormats")?;
        let mut formats = vec![0i64; count as usize];
        xr_err(unsafe { (xr.xrEnumerateSwapchainFormats)(handle, count, &mut count, formats.as_mut_ptr()) }, "xrEnumerateSwapchainFormats")?;
        formats.truncate(count as usize);
        // The frames are BGRA8 UNORM holding sRGB-encoded colour: copied bit for bit into an
        // sRGB image the compositor shows them as on a phone. A runtime without the BGRA
        // order gets a blit (which converts, and so is a shade lighter in an sRGB image).
        let pick = [(vk::Format::B8G8R8A8_SRGB, false), (vk::Format::B8G8R8A8_UNORM, false), (vk::Format::R8G8B8A8_SRGB, true), (vk::Format::R8G8B8A8_UNORM, true)];
        let (format, blit) = pick.into_iter().find(|(f, _)| formats.contains(&(f.as_raw() as i64)))
            .ok_or_else(|| format!("no 8-bit RGBA XR swapchain format among {formats:?}"))?;
        xr_log!("session {handle:#x}; swapchain formats {formats:?}: panel {format:?}{}", if blit { " (blit)" } else { "" });
        let panel = Self::chain(xr, handle, format, PANEL_PX, sys::SWAPCHAIN_USAGE_COLOR_ATTACHMENT | sys::SWAPCHAIN_USAGE_TRANSFER_DST | sys::SWAPCHAIN_USAGE_SAMPLED, 0)?;
        let cursor = Self::chain(xr, handle, format, [CURSOR_PX; 2], sys::SWAPCHAIN_USAGE_COLOR_ATTACHMENT | sys::SWAPCHAIN_USAGE_TRANSFER_DST | sys::SWAPCHAIN_USAGE_SAMPLED, sys::SWAPCHAIN_CREATE_STATIC_IMAGE)?;
        let mut slots = Vec::new();
        for _ in 0..panel.images.len() { slots.push(Self::slot(&dev)?); }
        let input = Input::new(xr, handle)?;
        Ok(Self { rt, dev, frames, handle, local, panel, cursor, slots, blit, input, running: false, focused: false, shown: None, cursor_ready: false })
    }

    fn chain(xr: &Xr, session: u64, format: vk::Format, size: [u32; 2], usage: u64, flags: u64) -> Result<Chain, String> {
        let info = sys::SwapchainCreateInfo { ty: Ty::SWAPCHAIN_CREATE_INFO, next: std::ptr::null(), create_flags: flags, usage_flags: usage, format: format.as_raw() as i64,
            sample_count: 1, width: size[0], height: size[1], face_count: 1, array_size: 1, mip_count: 1 };
        let mut handle = 0;
        xr_err(unsafe { (xr.xrCreateSwapchain)(session, &info, &mut handle) }, "xrCreateSwapchain")?;
        let mut count = 0;
        xr_err(unsafe { (xr.xrEnumerateSwapchainImages)(handle, 0, &mut count, std::ptr::null_mut()) }, "xrEnumerateSwapchainImages")?;
        let mut images = vec![sys::SwapchainImageVulkanKHR { ty: Ty::SWAPCHAIN_IMAGE_VULKAN_KHR, next: std::ptr::null_mut(), image: 0 }; count as usize];
        xr_err(unsafe { (xr.xrEnumerateSwapchainImages)(handle, count, &mut count, images.as_mut_ptr()) }, "xrEnumerateSwapchainImages")?;
        Ok(Chain { handle, images: images[..count as usize].iter().map(|i| vk::Image(i.image)).collect() })
    }

    fn slot(dev: &Device) -> Result<Slot, String> {
        unsafe {
            let pool = dev.raw.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(dev.family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None).map_err(|e| format!("XR command pool: {e:?}"))?;
            let cb = dev.raw.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool)
                .level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1)).map_err(|e| format!("XR command buffer: {e:?}"))?[0];
            let fence = dev.raw.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None).map_err(|e| format!("XR fence: {e:?}"))?;
            Ok(Slot { pool, cb, fence, keep: None })
        }
    }

    /// The XR frame loop, for the life of the process.
    fn run(mut self) {
        let mut said = false;
        loop {
            match self.poll() {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => { xr_log!("{e}"); break; }
            }
            if !self.running { std::thread::sleep(std::time::Duration::from_millis(10)); continue; }
            if let Err(e) = self.frame() {
                if !said { xr_log!("frame: {e}"); said = true; }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        xr_log!("the session ended; the XR thread stops");
    }

    /// Drains the runtime's events; `false` once the session is exiting or lost.
    fn poll(&mut self) -> Result<bool, String> {
        let rt = self.rt.clone();
        let xr = &rt.xr;
        loop {
            let mut ev = sys::EventDataBuffer { ty: Ty::EVENT_DATA_BUFFER, next: std::ptr::null(), varying: [0; 4000] };
            let r = unsafe { (xr.xrPollEvent)(xr.instance, &mut ev) };
            if r == sys::EVENT_UNAVAILABLE { return Ok(true); }
            xr_err(r, "xrPollEvent")?;
            if ev.ty == Ty::EVENT_DATA_INSTANCE_LOSS_PENDING { xr_log!("instance loss pending"); return Ok(false); }
            if ev.ty != Ty::EVENT_DATA_SESSION_STATE_CHANGED { continue; }
            let state = unsafe { &*(&ev as *const _ as *const sys::EventDataSessionStateChanged) }.state;
            xr_log!("session state {state}");
            use sys::session_state::*;
            match state {
                READY => {
                    let info = sys::SessionBeginInfo { ty: Ty::SESSION_BEGIN_INFO, next: std::ptr::null(), primary_view_configuration_type: sys::VIEW_CONFIGURATION_PRIMARY_STEREO };
                    xr_err(unsafe { (xr.xrBeginSession)(self.handle, &info) }, "xrBeginSession")?;
                    self.running = true;
                    if let Some(set_thread) = xr.xrSetAndroidApplicationThreadKHR {
                        extern "C" { fn gettid() -> i32; }
                        let r = unsafe { set_thread(self.handle, sys::ANDROID_THREAD_TYPE_RENDERER_MAIN, gettid() as u32) };
                        if r < 0 { xr_log!("xrSetAndroidApplicationThreadKHR: {r}"); }
                    }
                }
                STOPPING => {
                    self.running = false;
                    xr_err(unsafe { (xr.xrEndSession)(self.handle) }, "xrEndSession")?;
                }
                EXITING | LOSS_PENDING => return Ok(false),
                _ => {}
            }
            let focused = state == FOCUSED;
            if focused != self.focused {
                self.focused = focused;
                let shared = Shared::global();
                lock(&shared.state).focused = focused;
                shared.emit(clock_ns(), |q, id| { q.focus(id, focused); q.lifecycle(if focused { lifecycle::FOCUS_GAINED } else { lifecycle::FOCUS_LOST }); });
            }
        }
    }

    fn frame(&mut self) -> Result<(), String> {
        let rt = self.rt.clone();
        let xr = &rt.xr;
        let wait = sys::FrameWaitInfo { ty: Ty::FRAME_WAIT_INFO, next: std::ptr::null() };
        let mut fs = sys::FrameState { ty: Ty::FRAME_STATE, next: std::ptr::null_mut(), predicted_display_time: 0, predicted_display_period: 0, should_render: 0 };
        xr_err(unsafe { (xr.xrWaitFrame)(self.handle, &wait, &mut fs) }, "xrWaitFrame")?;
        let time = fs.predicted_display_time;
        let shared = Shared::global();
        if fs.predicted_display_period > 0 { lock(&shared.state).refresh = (1e9 / fs.predicted_display_period as f64) as f32; }
        // the app's frame for this display frame (XrTime is CLOCK_MONOTONIC on Android, the HAL clock)
        shared.frame(time as u64);
        {
            let _queue = lock(&self.dev.queue_lock);
            xr_err(unsafe { (xr.xrBeginFrame)(self.handle, &sys::FrameBeginInfo { ty: Ty::FRAME_BEGIN_INFO, next: std::ptr::null() }) }, "xrBeginFrame")?;
        }
        let hit = self.input(time);
        let done = if fs.should_render != 0 { self.update_panel()? } else { None };
        if !self.cursor_ready && fs.should_render != 0 { self.fill_cursor()?; }
        let quad = |swapchain, size: [u32; 2], pose, metres: [f32; 2]| sys::CompositionLayerQuad { ty: Ty::COMPOSITION_LAYER_QUAD, next: std::ptr::null(),
            layer_flags: sys::LAYER_BLEND_TEXTURE_SOURCE_ALPHA, space: self.local, eye_visibility: sys::EYE_VISIBILITY_BOTH,
            sub_image: sys::SwapchainSubImage { swapchain, image_rect: sys::Rect2Di { offset: sys::Offset2Di { x: 0, y: 0 }, extent: sys::Extent2Di { width: size[0] as i32, height: size[1] as i32 } }, image_array_index: 0 },
            pose, size: sys::Extent2Df { width: metres[0], height: metres[1] } };
        let panel_pose = Posef { orientation: Quat { x: 0., y: 0., z: 0., w: 1. }, position: Vec3 { x: 0., y: 0., z: PANEL_Z } };
        let panel = self.shown.map(|size| quad(self.panel.handle, size, panel_pose, PANEL_M));
        let cursor = hit.filter(|_| self.cursor_ready && self.shown.is_some()).map(|p| quad(self.cursor.handle, [CURSOR_PX; 2],
            Posef { orientation: panel_pose.orientation, position: Vec3 { x: p[0], y: p[1], z: PANEL_Z + 0.003 } }, [CURSOR_M; 2]));
        let mut layers: Vec<*const c_void> = Vec::new();
        if fs.should_render != 0 {
            if let Some(l) = &panel { layers.push(l as *const _ as *const c_void); }
            if let Some(l) = &cursor { layers.push(l as *const _ as *const c_void); }
        }
        let end = sys::FrameEndInfo { ty: Ty::FRAME_END_INFO, next: std::ptr::null(), display_time: time, environment_blend_mode: sys::ENVIRONMENT_BLEND_MODE_OPAQUE,
            layer_count: layers.len() as u32, layers: layers.as_ptr() };
        let r = { let _queue = lock(&self.dev.queue_lock); unsafe { (xr.xrEndFrame)(self.handle, &end) } };
        if let Some(done) = done { done(time as u64); }
        xr_err(r, "xrEndFrame")
    }

    /// Copies the newest app frame into the panel's next image; the frame's `presented`.
    fn update_panel(&mut self) -> Result<Option<Box<dyn FnOnce(u64) + Send>>, String> {
        let Some((image, done)) = lock(&self.frames.next).take() else { return Ok(None) };
        let rt = self.rt.clone();
        let xr = &rt.xr;
        let mut index = 0;
        {
            let _queue = lock(&self.dev.queue_lock);
            xr_err(unsafe { (xr.xrAcquireSwapchainImage)(self.panel.handle, &sys::SwapchainImageAcquireInfo { ty: Ty::SWAPCHAIN_IMAGE_ACQUIRE_INFO, next: std::ptr::null() }, &mut index) }, "xrAcquireSwapchainImage")?;
        }
        xr_err(unsafe { (xr.xrWaitSwapchainImage)(self.panel.handle, &sys::SwapchainImageWaitInfo { ty: Ty::SWAPCHAIN_IMAGE_WAIT_INFO, next: std::ptr::null(), timeout: i64::MAX }) }, "xrWaitSwapchainImage")?;
        let size = [image.desc.w.min(PANEL_PX[0]), image.desc.h.min(PANEL_PX[1])];
        let dst = self.panel.images[index as usize];
        let blit = self.blit;
        let recorded = self.record(index as usize, |dev, cb| unsafe {
            let sub = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
            if blit {
                let corner = vk::Offset3D { x: size[0] as i32, y: size[1] as i32, z: 1 };
                let region = vk::ImageBlit::default().src_subresource(sub).dst_subresource(sub)
                    .src_offsets([vk::Offset3D::default(), corner]).dst_offsets([vk::Offset3D::default(), corner]);
                dev.raw.cmd_blit_image(cb, image.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, dst, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region], vk::Filter::NEAREST);
            } else {
                let region = vk::ImageCopy::default().src_subresource(sub).dst_subresource(sub).extent(vk::Extent3D { width: size[0], height: size[1], depth: 1 });
                dev.raw.cmd_copy_image(cb, image.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, dst, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);
            }
        }, dst);
        // the frame image is read until the copy's fence (`Slot::keep`)
        if recorded.is_ok() { self.slots[index as usize].keep = Some(image); }
        let released = {
            let _queue = lock(&self.dev.queue_lock);
            unsafe { (xr.xrReleaseSwapchainImage)(self.panel.handle, &sys::SwapchainImageReleaseInfo { ty: Ty::SWAPCHAIN_IMAGE_RELEASE_INFO, next: std::ptr::null() }) }
        };
        recorded?;
        xr_err(released, "xrReleaseSwapchainImage")?;
        if self.shown.is_none() { xr_log!("first frame on the panel ({}x{} px)", size[0], size[1]); }
        self.shown = Some(size);
        Ok(Some(done))
    }

    /// Records `body` between the barriers that take `dst` (an acquired XR image, in
    /// COLOR_ATTACHMENT_OPTIMAL) to a transfer destination and back, and submits it with slot
    /// `index`'s fence. Earlier submissions on the queue (the app's frame) are ordered before it.
    fn record(&mut self, index: usize, body: impl FnOnce(&Device, vk::CommandBuffer), dst: vk::Image) -> Result<(), String> {
        let dev = &self.dev;
        let slot = &mut self.slots[index];
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let barrier = |old, new, src, dest| vk::ImageMemoryBarrier::default().image(dst).old_layout(old).new_layout(new)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .src_access_mask(src).dst_access_mask(dest).subresource_range(range);
        unsafe {
            dev.raw.wait_for_fences(&[slot.fence], true, u64::MAX).map_err(|e| format!("XR copy fence: {e:?}"))?;
            slot.keep = None;
            dev.raw.reset_fences(&[slot.fence]).map_err(|e| format!("XR copy fence reset: {e:?}"))?;
            dev.raw.reset_command_buffer(slot.cb, vk::CommandBufferResetFlags::empty()).map_err(|e| format!("XR copy reset: {e:?}"))?;
            dev.raw.begin_command_buffer(slot.cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(|e| format!("XR copy begin: {e:?}"))?;
            dev.raw.cmd_pipeline_barrier(slot.cb, vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::MEMORY_WRITE).dst_access_mask(vk::AccessFlags::TRANSFER_READ)], &[],
                &[barrier(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::COLOR_ATTACHMENT_WRITE, vk::AccessFlags::TRANSFER_WRITE)]);
            body(dev, slot.cb);
            dev.raw.cmd_pipeline_barrier(slot.cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, vk::DependencyFlags::empty(), &[], &[],
                &[barrier(vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE)]);
            dev.raw.end_command_buffer(slot.cb).map_err(|e| format!("XR copy end: {e:?}"))?;
            let cbs = [slot.cb];
            let _queue = lock(&dev.queue_lock);
            dev.raw.queue_submit(dev.queue, &[vk::SubmitInfo::default().command_buffers(&cbs)], slot.fence).map_err(|e| format!("XR copy submit: {e:?}"))?;
        }
        Ok(())
    }

    /// The pointer dot's one (static) image: opaque white.
    fn fill_cursor(&mut self) -> Result<(), String> {
        let rt = self.rt.clone();
        let xr = &rt.xr;
        let mut index = 0;
        {
            let _queue = lock(&self.dev.queue_lock);
            xr_err(unsafe { (xr.xrAcquireSwapchainImage)(self.cursor.handle, &sys::SwapchainImageAcquireInfo { ty: Ty::SWAPCHAIN_IMAGE_ACQUIRE_INFO, next: std::ptr::null() }, &mut index) }, "xrAcquireSwapchainImage (pointer)")?;
        }
        xr_err(unsafe { (xr.xrWaitSwapchainImage)(self.cursor.handle, &sys::SwapchainImageWaitInfo { ty: Ty::SWAPCHAIN_IMAGE_WAIT_INFO, next: std::ptr::null(), timeout: i64::MAX }) }, "xrWaitSwapchainImage (pointer)")?;
        let dst = self.cursor.images[index as usize];
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        // slot 0's fence orders it like a panel copy; it is waited before that slot is used again
        let recorded = self.record(0, |dev, cb| unsafe {
            dev.raw.cmd_clear_color_image(cb, dst, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &vk::ClearColorValue { float32: [1., 1., 1., 1.] }, &[range]);
        }, dst);
        let released = {
            let _queue = lock(&self.dev.queue_lock);
            unsafe { (xr.xrReleaseSwapchainImage)(self.cursor.handle, &sys::SwapchainImageReleaseInfo { ty: Ty::SWAPCHAIN_IMAGE_RELEASE_INFO, next: std::ptr::null() }) }
        };
        recorded?;
        xr_err(released, "xrReleaseSwapchainImage (pointer)")?;
        self.cursor_ready = true;
        Ok(())
    }

    /// Syncs the controllers and turns the active hand's aim ray into mouse events on the panel.
    /// The hit point on the panel plane (local space, metres) while the ray meets the panel.
    fn input(&mut self, time: i64) -> Option<[f32; 2]> {
        let rt = self.rt.clone();
        let xr = &rt.xr;
        let input = &mut self.input;
        let active = [sys::ActiveActionSet { action_set: input.set, subaction_path: 0 }];
        let sync = sys::ActionsSyncInfo { ty: Ty::ACTIONS_SYNC_INFO, next: std::ptr::null(), count_active_action_sets: 1, active_action_sets: active.as_ptr() };
        if unsafe { (xr.xrSyncActions)(self.handle, &sync) } != sys::SUCCESS { return None; } // not focused
        let mut pressed = [false; 2];
        for (i, hand) in input.hands.iter().enumerate() {
            let get = sys::ActionStateGetInfo { ty: Ty::ACTION_STATE_GET_INFO, next: std::ptr::null(), action: input.select, subaction_path: hand.path };
            let mut state = sys::ActionStateBoolean { ty: Ty::ACTION_STATE_BOOLEAN, next: std::ptr::null_mut(), current_state: 0, changed_since_last_sync: 0, last_change_time: 0, is_active: 0 };
            if unsafe { (xr.xrGetActionStateBoolean)(self.handle, &get, &mut state) } == sys::SUCCESS && state.is_active != 0 {
                pressed[i] = state.current_state != 0;
                // a press of the other hand's trigger makes it the pointer (not mid-drag)
                if pressed[i] && state.changed_since_last_sync != 0 && !input.pressed { input.active = i; }
            }
        }
        let hand = &input.hands[input.active];
        let mut loc = sys::SpaceLocation { ty: Ty::SPACE_LOCATION, next: std::ptr::null_mut(), location_flags: 0, pose: Posef::IDENTITY };
        let located = unsafe { (xr.xrLocateSpace)(hand.space, self.local, time, &mut loc) } == sys::SUCCESS
            && loc.location_flags & (sys::SPACE_LOCATION_ORIENTATION_VALID | sys::SPACE_LOCATION_POSITION_VALID) == (sys::SPACE_LOCATION_ORIENTATION_VALID | sys::SPACE_LOCATION_POSITION_VALID);
        let hit = located.then(|| panel_hit(loc.pose)).flatten();
        let points = [PANEL_PX[0] as f64 / PANEL_DPI as f64, PANEL_PX[1] as f64 / PANEL_DPI as f64];
        let at = hit.map(|[u, v, ..]| [u as f64 * points[0], v as f64 * points[1]]);
        let inside = at.is_some_and(|[x, y]| (0. ..=points[0]).contains(&x) && (0. ..=points[1]).contains(&y));
        let down = pressed[input.active];
        let shared = Shared::global();
        let now = clock_ns();
        if let Some(pos @ [x, y]) = at.filter(|_| inside || input.pressed) {
            if input.at != Some(pos) { shared.emit(now, |q, id| q.mouse_move(id, x, y, 0., 0., 0)); }
            if down && !input.pressed { shared.emit(now, |q, id| q.mouse_down(id, x, y, 1, 0)); }
            if !down && input.pressed { shared.emit(now, |q, id| q.mouse_up(id, x, y, 1, 0)); }
            input.at = Some(pos);
        } else {
            if input.pressed && !down {
                let [x, y] = input.at.unwrap_or_default();
                shared.emit(now, |q, id| q.mouse_up(id, x, y, 1, 0));
            }
            if input.inside {
                let [x, y] = input.at.unwrap_or_default();
                shared.emit(now, |q, id| q.mouse_leave(id, x, y));
            }
            input.at = None;
        }
        input.pressed = down && (inside || input.pressed);
        input.inside = inside;
        if let Some([x, y]) = input.at.filter(|_| inside) {
            let get = sys::ActionStateGetInfo { ty: Ty::ACTION_STATE_GET_INFO, next: std::ptr::null(), action: input.scroll, subaction_path: hand.path };
            let mut stick = sys::ActionStateVector2f { ty: Ty::ACTION_STATE_VECTOR2F, next: std::ptr::null_mut(), current_state: sys::Vec2::default(), changed_since_last_sync: 0, last_change_time: 0, is_active: 0 };
            if unsafe { (xr.xrGetActionStateVector2f)(self.handle, &get, &mut stick) } == sys::SUCCESS && stick.is_active != 0 {
                let s = stick.current_state;
                let axis = |v: f32| if v.abs() < STICK_DEAD { 0. } else { (v * STICK_SCROLL) as f64 };
                let (dx, dy) = (axis(s.x), axis(-s.y));
                if dx != 0. || dy != 0. { shared.emit(now, |q, id| q.scroll(id, x, y, dx, dy, 0, true, 0)); }
            }
        }
        hit.filter(|_| inside).map(|[_, _, px, py]| [px, py])
    }
}

/// Where a ray from `pose` (along its -Z) meets the panel: (u, v) across it from its top-left
/// corner (0..1 on it) and the point (x, y) on its plane; `None` when it points away.
fn panel_hit(pose: Posef) -> Option<[f32; 4]> {
    let q = pose.orientation;
    let o = pose.position;
    // v' = v + w t + q x t with t = 2 (q x v), v = (0, 0, -1)
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let qv = [q.x, q.y, q.z];
    let t = cross(qv, [0., 0., -1.]).map(|c| 2. * c);
    let c = cross(qv, t);
    let d = [q.w * t[0] + c[0], q.w * t[1] + c[1], -1. + q.w * t[2] + c[2]];
    if d[2] > -1e-4 { return None; }
    let s = (PANEL_Z - o.z) / d[2];
    if s < 0. { return None; }
    let (x, y) = (o.x + s * d[0], o.y + s * d[1]);
    Some([(x + PANEL_M[0] / 2.) / PANEL_M[0], (PANEL_M[1] / 2. - y) / PANEL_M[1], x, y])
}

impl Input {
    fn new(xr: &Xr, session: u64) -> Result<Self, String> {
        let info = sys::ActionSetCreateInfo { ty: Ty::ACTION_SET_CREATE_INFO, next: std::ptr::null(), action_set_name: sys::fixed("pointer"), localized_action_set_name: sys::fixed("Pointer"), priority: 0 };
        let mut set = 0;
        xr_err(unsafe { (xr.xrCreateActionSet)(xr.instance, &info, &mut set) }, "xrCreateActionSet")?;
        let hands = [xr.path(c"/user/hand/left")?, xr.path(c"/user/hand/right")?];
        let action = |name: &str, ty: i32| -> Result<u64, String> {
            let info = sys::ActionCreateInfo { ty: Ty::ACTION_CREATE_INFO, next: std::ptr::null(), action_name: sys::fixed(name), action_type: ty,
                count_subaction_paths: 2, subaction_paths: hands.as_ptr(), localized_action_name: sys::fixed(name) };
            let mut a = 0;
            xr_err(unsafe { (xr.xrCreateAction)(set, &info, &mut a) }, "xrCreateAction")?;
            Ok(a)
        };
        let aim = action("aim", sys::ACTION_TYPE_POSE_INPUT)?;
        let select = action("select", sys::ACTION_TYPE_BOOLEAN_INPUT)?;
        let scroll = action("scroll", sys::ACTION_TYPE_VECTOR2F_INPUT)?;
        let suggest = |profile: &CStr, bindings: &[(u64, &CStr)]| -> Result<(), String> {
            let b = bindings.iter().map(|(a, p)| Ok(sys::ActionSuggestedBinding { action: *a, binding: xr.path(p)? })).collect::<Result<Vec<_>, String>>()?;
            let info = sys::InteractionProfileSuggestedBinding { ty: Ty::INTERACTION_PROFILE_SUGGESTED_BINDING, next: std::ptr::null(), interaction_profile: xr.path(profile)?,
                count_suggested_bindings: b.len() as u32, suggested_bindings: b.as_ptr() };
            xr_err(unsafe { (xr.xrSuggestInteractionProfileBindings)(xr.instance, &info) }, "xrSuggestInteractionProfileBindings")
        };
        suggest(c"/interaction_profiles/oculus/touch_controller", &[
            (aim, c"/user/hand/left/input/aim/pose"), (aim, c"/user/hand/right/input/aim/pose"),
            (select, c"/user/hand/left/input/trigger/value"), (select, c"/user/hand/right/input/trigger/value"),
            (scroll, c"/user/hand/left/input/thumbstick"), (scroll, c"/user/hand/right/input/thumbstick")])?;
        suggest(c"/interaction_profiles/khr/simple_controller", &[
            (aim, c"/user/hand/left/input/aim/pose"), (aim, c"/user/hand/right/input/aim/pose"),
            (select, c"/user/hand/left/input/select/click"), (select, c"/user/hand/right/input/select/click")])?;
        let sets = [set];
        let attach = sys::SessionActionSetsAttachInfo { ty: Ty::SESSION_ACTION_SETS_ATTACH_INFO, next: std::ptr::null(), count_action_sets: 1, action_sets: sets.as_ptr() };
        xr_err(unsafe { (xr.xrAttachSessionActionSets)(session, &attach) }, "xrAttachSessionActionSets")?;
        let space = |path: u64| -> Result<Hand, String> {
            let info = sys::ActionSpaceCreateInfo { ty: Ty::ACTION_SPACE_CREATE_INFO, next: std::ptr::null(), action: aim, subaction_path: path, pose_in_action_space: Posef::IDENTITY };
            let mut space = 0;
            xr_err(unsafe { (xr.xrCreateActionSpace)(session, &info, &mut space) }, "xrCreateActionSpace")?;
            Ok(Hand { path, space })
        };
        Ok(Self { set, select, scroll, hands: [space(hands[0])?, space(hands[1])?], active: 1, pressed: false, at: None, inside: false })
    }
}
