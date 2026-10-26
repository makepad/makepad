//! Immersive Quest builds (the APK carries `libopenxr_loader.so`): the retired Android OpenXR
//! backend (`platform/src/os/linux/openxr*.rs` before c6a5ddbf4b), refactored into the HAL.
//!
//! What stayed the old backend's: the instance (its extension list and checks), the session and
//! its spaces (LOCAL_FLOOR, falling back to STAGE and LOCAL), passthrough and environment depth
//! made with the session, performance levels and thread hints at begin, the refresh-rate and
//! reference-space events, the frame (xrWaitFrame, head and eyes located through the VIEW
//! space), the controllers and hands (`input`), and the layers (passthrough under a projection
//! that blends with source alpha). `sys` is its OpenXR ABI, whole.
//!
//! What the HAL provides instead of the old glue: the Vulkan instance and device come from the
//! core through [`VkCreator`] (XR_KHR_vulkan_enable2); the XR thread paces every window with
//! xrWaitFrame instead of the Choreographer; and Cx no longer draws into the swapchain itself.
//! Two kinds of window show in the headset, both through the core's [`FramePresenter`] seam:
//! - the host window is a panel: a quad layer in front of the person; controller aim rays and
//!   hand pinch rays are mouse events on it;
//! - a `window_kind::XR` window is the stereo view: both eyes side by side. The XR thread sends it
//!   `XrFrame` (head, eyes, controllers and hands at the predicted display time) and
//!   FrameReady, waits for that frame within the display frame, and copies each eye into its
//!   layer of the projection swapchain, submitted with the eyes it was drawn with.
mod input;
mod sys;
mod vulkan;

use self::input::CxOpenXrInputs;
use self::sys::*;
use super::{jni, shell::{lock, Shared}};
use crate::backend::vulkan::surface::{FramePresenter, SurfaceState, VkCreator, VkSurfaceSource};
use crate::backend::vulkan::{Device, Image};
use crate::{clock_ns, lifecycle, session::HOST, window_state, xr_controller, xr_hand, Id, XrController, XrEye, XrFrame, XrHand, XrPose};
use makepad_vulkan::{commands::PfnGetInstanceProcAddr, vk};
use std::ffi::{c_void, CStr};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The old backend's options (`CxOpenXrOptions`). The eye buffers are the recommended size
/// (the old default scaled them by 1.4 for a light scene; Sandbox under the JIT renders whole
/// games). Multisampling and foveation apply where the swapchain is rendered into directly,
/// which the HAL does not do yet (it copies finished frames).
#[derive(Clone, Copy)]
#[allow(dead_code)] // multisamples and foveation: with rendering straight into the swapchain
pub struct CxOpenXrOptions {
    pub buffer_scale: f32,
    pub multisamples: usize,
    pub remove_hands_from_depth: bool,
    pub fixed_foveation_level: u8,
}
const XR_OPTIONS: CxOpenXrOptions = CxOpenXrOptions { buffer_scale: 1.0, multisamples: 1, remove_hands_from_depth: true, fixed_foveation_level: 3 };

/// The panel: its pixels, their density (window points = pixels / density), its size in
/// metres and where it hangs: `PANEL_Z` ahead of where the head was when the session began, at
/// that height.
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
/// The trigger pull that is a click on the panel.
const TRIGGER_PRESS: f32 = 0.5;
/// How long before its display time a stereo frame may still arrive for that display frame.
const STEREO_MARGIN_NS: i64 = 4_000_000;
/// A stereo frame asked for and not delivered within this is asked again.
const STEREO_REASK: Duration = Duration::from_millis(250);

static ACTIVE: AtomicBool = AtomicBool::new(false);
/// The eye size (pixels): an XR window is twice as wide.
static EYE_PX: OnceLock<[u32; 2]> = OnceLock::new();

/// An XR session shows the windows (the Choreographer and the Activity's surface and window
/// focus are not used).
pub(super) fn active() -> bool { ACTIVE.load(Ordering::Acquire) }

/// XR lines go straight to logcat (tag `hal-xr`), live, whatever stderr is redirected to.
pub(super) fn log(text: &str) {
    extern "C" { fn __android_log_write(prio: i32, tag: *const std::ffi::c_char, text: *const std::ffi::c_char) -> i32; }
    if let Ok(t) = std::ffi::CString::new(text.replace('\0', " ")) { unsafe { __android_log_write(4, c"hal-xr".as_ptr(), t.as_ptr()) }; }
}
macro_rules! xr_log { ($($t:tt)*) => { log(&format!($($t)*)) } }

/// `window_kind::XR` window `id` of session `sid` is made: it is the stereo view, sized by the
/// eyes (one pixel per point). `false` without an XR session.
pub(super) fn create_window(sid: u32, id: Id) -> bool {
    let Some(eye) = EYE_PX.get().copied().filter(|_| active()) else { return false };
    let shared = Shared::global();
    let previous = lock(&shared.registry).xr_window.replace((sid, id));
    if previous.is_some() { xr_log!("a second XR window replaces {previous:?}"); }
    let refresh = lock(&shared.state).refresh;
    shared.emit_to(sid, id, clock_ns(), |q, id| {
        q.display(id, 1., refresh);
        q.geom(id, 0., 0., 2. * eye[0] as f64, eye[1] as f64, 1., window_state::RESTORE);
    });
    xr_log!("XR window {id:?} of session {sid}: {}x{} px", 2 * eye[0], eye[1]);
    true
}

/// The loader, the instance and its system (the old `CxOpenXr`).
struct Runtime { _loader: LibOpenXrLoader, xr: LibOpenXr, instance: XrInstance, system: XrSystemId, app_thread: u32 }
unsafe impl Send for Runtime {}
unsafe impl Sync for Runtime {}

extern "C" { fn gettid() -> i32; }

impl Runtime {
    /// The old `CxOpenXr::create_instance` (Vulkan variant): loader init with the Activity, the
    /// extensions it asked for, the instance, its system, and the Vulkan graphics requirements
    /// and recommended eye size the HAL needs before it makes a device or an XR window.
    fn create_instance(vm: *mut c_void, activity: *mut c_void) -> Result<Runtime, String> {
        let loader = LibOpenXrLoader::try_load()?;
        let loader_info = XrLoaderInitInfoAndroidKHR { ty: XrStructureType::LOADER_INIT_INFO_ANDROID_KHR, next: ptr::null(), application_vm: vm, application_context: activity };
        unsafe { (loader.xrInitializeLoaderKHR)(&loader_info as *const _ as _) }.to_result("xrInitializeLoaderKHR")?;

        let exts = xr_array_fetch(XrExtensionProperties::default(), |cap, len, buf| {
            unsafe { (loader.xrEnumerateInstanceExtensionProperties)(ptr::null(), cap, len, buf) }.to_result("xrEnumerateInstanceExtensionProperties")
        })?;
        let has_extension = |name: &'static str| exts.iter().any(|e| xr_string_zero_terminated(&e.extension_name) == name);

        let mut exts_needed = vec![
            "XR_EXT_performance_settings\0",
            "XR_EXT_hand_tracking\0",
            "XR_EXT_hand_interaction\0",
            "XR_KHR_android_thread_settings\0",
            "XR_FB_passthrough\0",
            "XR_META_environment_depth\0",
            "XR_META_touch_controller_plus\0",
            "XR_META_detached_controllers\0",
            "XR_META_simultaneous_hands_and_controllers\0",
            "XR_FB_hand_tracking_mesh\0",
            "XR_FB_hand_tracking_aim\0",
            "XR_META_colocation_discovery\0",
            "XR_META_spatial_entity_persistence\0",
            "XR_META_spatial_entity_sharing\0",
            "XR_META_spatial_entity_group_sharing\0",
            "XR_FB_spatial_entity\0",
            "XR_FB_spatial_entity_query\0",
        ];
        let fixed_foveation_exts = ["XR_FB_swapchain_update_state\0", "XR_FB_foveation\0", "XR_FB_foveation_configuration\0", "XR_FB_foveation_vulkan\0"];
        let missing_fixed_foveation_exts: Vec<&str> = fixed_foveation_exts.iter().copied().filter(|name| !has_extension(name)).collect();
        if missing_fixed_foveation_exts.is_empty() {
            exts_needed.extend_from_slice(&fixed_foveation_exts);
        } else {
            xr_log!("OpenXR fixed foveation extensions unavailable on this runtime: {:?}", missing_fixed_foveation_exts.iter().map(|e| e.trim_end_matches(char::from(0))).collect::<Vec<_>>());
        }
        let display_refresh_rate_ext = "XR_FB_display_refresh_rate\0";
        if has_extension(display_refresh_rate_ext) {
            exts_needed.push(display_refresh_rate_ext);
        } else {
            xr_log!("OpenXR display refresh rate extension unavailable on this runtime");
        }
        if !has_extension("XR_KHR_vulkan_enable2\0") {
            return Err("OpenXR Vulkan: XR_KHR_vulkan_enable2 is required on this Quest path".to_string());
        }
        exts_needed.insert(0, "XR_KHR_vulkan_enable2\0");

        let missing_exts: Vec<&str> = exts_needed.iter().copied().filter(|name| !has_extension(name)).collect();
        if !missing_exts.is_empty() {
            // Horizon OS hides extensions whose manifest permission is missing; asking for
            // one fails the instance, so they are left out (their features stay off)
            xr_log!("OpenXR runtime missing requested extensions (left out): {:?}", missing_exts.iter().map(|e| e.trim_end_matches(char::from(0))).collect::<Vec<_>>());
            exts_needed.retain(|name| has_extension(name));
        }
        let ext_name_ptrs: Vec<*const std::os::raw::c_char> = exts_needed.iter().map(|ext| ext.as_ptr() as *const std::os::raw::c_char).collect();
        let create_info = XrInstanceCreateInfo {
            ty: XrStructureType::INSTANCE_CREATE_INFO,
            next: ptr::null(),
            create_flags: XrInstanceCreateFlags(0),
            application_info: XrApplicationInfo {
                application_name: xr_to_string("makepad"),
                application_version: 0,
                engine_name: xr_to_string("Makepad"),
                engine_version: 0,
                api_version: XP_API_VERSION_1_0,
            },
            enabled_api_layer_count: 0,
            enabled_api_layer_names: ptr::null(),
            enabled_extension_count: exts_needed.len() as u32,
            enabled_extension_names: ext_name_ptrs.as_ptr(),
        };
        let mut instance = XrInstance(0);
        unsafe { (loader.xrCreateInstance)(&create_info, &mut instance) }.to_result("xrCreateInstance")?;

        let xr = match LibOpenXr::try_load(&loader, instance) {
            Ok(xr) => xr,
            Err(err) => { loader.destroy_instance(instance); return Err(err); }
        };
        let system = (|| {
            let mut instance_props = XrInstanceProperties::default();
            unsafe { (xr.xrGetInstanceProperties)(instance, &mut instance_props) }.to_result("xrGetInstanceProperties")?;
            let mut sys_info = XrSystemGetInfo::default();
            sys_info.form_factor = XrFormFactor::HEAD_MOUNTED_DISPLAY;
            let mut sys_id = XrSystemId(0);
            unsafe { (xr.xrGetSystem)(instance, &mut sys_info, &mut sys_id) }.to_result("xrGetSystem")?;
            let mut sys_props = XrSystemProperties::default();
            unsafe { (xr.xrGetSystemProperties)(instance, sys_id, &mut sys_props) }.to_result("xrGetSystemProperties")?;
            // required before the runtime makes a Vulkan device
            let mut req = XrGraphicsRequirementsVulkanKHR::default();
            unsafe { (xr.xrGetVulkanGraphicsRequirements2KHR)(instance, sys_id, &mut req) }.to_result("xrGetVulkanGraphicsRequirements2KHR")?;
            let config_views = xr_array_fetch(XrViewConfigurationView::default(), |cap, len, buf| {
                unsafe { (xr.xrEnumerateViewConfigurationViews)(instance, sys_id, XrViewConfigurationType::PRIMARY_STEREO, cap, len, buf) }.to_result("xrEnumerateViewConfigurationViews")
            })?;
            let view = config_views.first().ok_or("no PRIMARY_STEREO views")?;
            let scale = |v: u32| ((v as f32) * XR_OPTIONS.buffer_scale).max(1.0) as u32;
            let eye = [scale(view.recommended_image_rect_width), scale(view.recommended_image_rect_height)];
            let _ = EYE_PX.set(eye);
            xr_log!("instance {instance:?} system {sys_id:?} runtime {} {:?}; extensions {:?}; eye {}x{} px", xr_string_zero_terminated(&instance_props.runtime_name).trim_end_matches(char::from(0)),
                instance_props.runtime_version, exts_needed.iter().map(|e| e.trim_end_matches('\0')).collect::<Vec<_>>(), eye[0], eye[1]);
            Ok::<_, String>(sys_id)
        })();
        match system {
            Ok(system) => Ok(Runtime { _loader: loader, xr, instance, system, app_thread: unsafe { gettid() } as u32 }),
            Err(e) => { unsafe { (xr.xrDestroyInstance)(instance) }.log_error("xrDestroyInstance"); Err(e) }
        }
    }
}

impl VkCreator for Runtime {
    unsafe fn create_instance(&self, gipa: PfnGetInstanceProcAddr, info: &vk::InstanceCreateInfo) -> Result<vk::Instance, String> {
        let ci = XrVulkanInstanceCreateInfoKHR { ty: XrStructureType::VULKAN_INSTANCE_CREATE_INFO_KHR, next: ptr::null(), system_id: self.system, create_flags: XrVulkanInstanceCreateFlagsKHR(0),
            pfn_get_instance_proc_addr: Some(std::mem::transmute::<PfnGetInstanceProcAddr, VkGetInstanceProcAddr>(gipa)),
            vulkan_create_info: info as *const _ as *const c_void, vulkan_allocator: ptr::null() };
        let (mut handle, mut result): (VkInstance, VkResult) = (ptr::null(), 0);
        (self.xr.xrCreateVulkanInstanceKHR)(self.instance, &ci, &mut handle, &mut result).to_result("xrCreateVulkanInstanceKHR")?;
        if result != 0 { return Err(format!("vkCreateInstance (through OpenXR): {:?}", vk::Result::from_raw(result))); }
        Ok(vk::Instance(handle as usize))
    }
    unsafe fn physical_device(&self, instance: vk::Instance) -> Result<vk::PhysicalDevice, String> {
        let info = XrVulkanGraphicsDeviceGetInfoKHR { ty: XrStructureType::VULKAN_GRAPHICS_DEVICE_GET_INFO_KHR, next: ptr::null(), system_id: self.system, vulkan_instance: instance.0 as VkInstance };
        let mut physical: VkPhysicalDevice = ptr::null();
        (self.xr.xrGetVulkanGraphicsDevice2KHR)(self.instance, &info, &mut physical).to_result("xrGetVulkanGraphicsDevice2KHR")?;
        Ok(vk::PhysicalDevice(physical as usize))
    }
    unsafe fn create_device(&self, gipa: PfnGetInstanceProcAddr, physical: vk::PhysicalDevice, info: &vk::DeviceCreateInfo) -> Result<vk::Device, vk::Result> {
        let ci = XrVulkanDeviceCreateInfoKHR { ty: XrStructureType::VULKAN_DEVICE_CREATE_INFO_KHR, next: ptr::null(), system_id: self.system, create_flags: XrVulkanDeviceCreateFlagsKHR(0),
            pfn_get_instance_proc_addr: Some(std::mem::transmute::<PfnGetInstanceProcAddr, VkGetInstanceProcAddr>(gipa)),
            vulkan_physical_device: physical.0 as VkPhysicalDevice, vulkan_create_info: info as *const _ as *const c_void, vulkan_allocator: ptr::null() };
        let (mut handle, mut result): (VkDevice, VkResult) = (ptr::null(), 0);
        if (self.xr.xrCreateVulkanDeviceKHR)(self.instance, &ci, &mut handle, &mut result).log_error("xrCreateVulkanDeviceKHR") { return Err(vk::Result::ERROR_INITIALIZATION_FAILED); }
        if result != 0 { return Err(vk::Result::from_raw(result)); }
        Ok(vk::Device(handle as usize))
    }
}

/// A finished frame waiting for the XR thread, and what to call when it is shown.
type Pending = (Arc<Image>, Box<dyn FnOnce(u64) + Send>);
/// The newest panel frame and the stereo window's frame in flight; `arrived` wakes the XR
/// thread waiting for a stereo frame.
#[derive(Default)]
struct Frames { panel: Mutex<Option<Pending>>, stereo: Mutex<Option<Pending>>, arrived: Condvar }

struct Presenter { frames: Arc<Frames>, stereo: bool }
impl FramePresenter for Presenter {
    fn submit(&self, _: u32, _: Id, image: Arc<Image>, presented: Box<dyn FnOnce(u64) + Send>) {
        let slot = if self.stereo { &self.frames.stereo } else { &self.frames.panel };
        let replaced = lock(slot).replace((image, presented));
        self.frames.arrived.notify_all();
        // a frame the XR thread never copied: it is over all the same (the app's frames in
        // flight are counted down)
        if let Some((_, done)) = replaced { done(clock_ns()); }
    }
}

pub struct XrSurfaces { rt: Arc<Runtime>, frames: Arc<Frames>, panel: Arc<dyn FramePresenter>, stereo: Arc<dyn FramePresenter>, started: bool }

impl XrSurfaces {
    /// `None` in an APK without the OpenXR loader (not a Quest build); else the instance and
    /// system (before any Vulkan object: the runtime makes those).
    pub fn try_new() -> Option<Result<Self, String>> {
        if ModuleLoader::load("libopenxr_loader.so").is_err() { return None; }
        Some((|| {
            let (vm, activity) = jni::vm_and_activity().ok_or("OpenXR needs the Activity, which is not created yet")?;
            let rt = Runtime::create_instance(vm, activity).inspect_err(|e| xr_log!("no XR session: {e}"))?;
            ACTIVE.store(true, Ordering::Release);
            lock(&Shared::global().state).xr = Some((PANEL_PX, PANEL_DPI));
            Shared::global().display();
            let frames = Arc::new(Frames::default());
            Ok(Self { rt: Arc::new(rt), panel: Arc::new(Presenter { frames: frames.clone(), stereo: false }),
                stereo: Arc::new(Presenter { frames: frames.clone(), stereo: true }), frames, started: false })
        })())
    }
}

impl VkSurfaceSource for XrSurfaces {
    fn instance_extensions(&self) -> Vec<&'static CStr> { Vec::new() }
    fn creator(&self) -> Option<Arc<dyn VkCreator>> { Some(self.rt.clone()) }
    fn device_ready(&mut self, dev: Arc<Device>) -> Result<(), String> {
        if self.started {
            // TODO(QUESTXR2): a device made after a loss needs a new session on it
            xr_log!("a replacement Vulkan device: the XR session stays on the lost one");
            return Ok(());
        }
        let session = CxOpenXrSession::create_session(self.rt.clone(), dev, self.frames.clone(), XR_OPTIONS)?;
        std::thread::Builder::new().name("hal-xr".into()).spawn(move || session.run()).map_err(|e| format!("XR thread: {e}"))?;
        self.started = true;
        Ok(())
    }
    fn presenter(&self, session: u32, window: Id) -> Option<Arc<dyn FramePresenter>> {
        let r = lock(&Shared::global().registry);
        if r.xr_window == Some((session, window)) { Some(self.stereo.clone()) }
        else if session == HOST && r.host == Some(window) { Some(self.panel.clone()) }
        else { None }
    }
    fn state(&mut self, session: u32, window: Id, _: [u32; 2]) -> SurfaceState {
        let r = lock(&Shared::global().registry);
        if r.xr_window == Some((session, window)) || (session == HOST && r.host == Some(window)) { SurfaceState::Managed } else { SurfaceState::Lost }
    }
}

/// The copy of a frame into one image of a chain: its command buffer and fence, and the frame
/// image read until that fence.
struct Slot { pool: vk::CommandPool, cb: vk::CommandBuffer, fence: vk::Fence, keep: Option<Arc<Image>> }

/// An XR swapchain, its Vulkan images, a copy slot per image, its size and layers.
struct Chain { handle: XrSwapchain, images: Vec<vk::Image>, slots: Vec<Slot>, size: [u32; 2], layers: u32, staging: Option<Arc<Image>> }

impl Chain {
    /// `swizzle`: the format is RGBA ordered, the frames BGRA: copies go through a staging image
    /// (the size of the frames copied in: `layers` side by side).
    fn new(xr: &LibOpenXr, dev: &Arc<Device>, session: XrSession, format: vk::Format, size: [u32; 2], layers: u32, flags: XrSwapchainCreateFlags, swizzle: bool) -> Result<Chain, String> {
        let (handle, images) = vulkan::create_swapchain(xr, session, format, size[0], size[1], layers, flags)?;
        let slots = images.iter().map(|_| Slot::new(dev)).collect::<Result<_, _>>()?;
        let staging = if swizzle {
            Some(Image::new(dev.clone(), crate::session::TextureObj { format: crate::format::RGBA8, w: size[0] * layers, h: size[1], depth: 1, mips: 1,
                usage: crate::usage::RENDER_TARGET | crate::usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None })?)
        } else { None };
        Ok(Chain { handle, images, slots, size, layers, staging })
    }

    /// Acquires the chain's next image, records `body` into it (between the barriers that make it
    /// a transfer destination and give it back in COLOR_ATTACHMENT_OPTIMAL), submits and releases
    /// it. `keep` is read by the commands until their fence.
    fn write(&mut self, xr: &LibOpenXr, dev: &Device, keep: Option<Arc<Image>>, body: impl FnOnce(&Device, vk::CommandBuffer, vk::Image)) -> Result<(), String> {
        let mut index = 0;
        {
            let _queue = lock(&dev.queue_lock);
            unsafe { (xr.xrAcquireSwapchainImage)(self.handle, &XrSwapchainImageAcquireInfo::default(), &mut index) }.to_result("xrAcquireSwapchainImage")?;
        }
        let wait_info = XrSwapchainImageWaitInfo { timeout: XrDuration(1000000000), ..Default::default() };
        loop {
            if unsafe { (xr.xrWaitSwapchainImage)(self.handle, &wait_info) } != XrResult::TIMEOUT_EXPIRED { break; }
            xr_log!("retry xrWaitSwapchainImage");
        }
        let dst = self.images[index as usize];
        let layers = self.layers;
        let recorded = self.slots[index as usize].record(dev, dst, layers, |dev, cb| body(dev, cb, dst));
        if recorded.is_ok() { self.slots[index as usize].keep = keep; }
        let released = {
            let _queue = lock(&dev.queue_lock);
            unsafe { (xr.xrReleaseSwapchainImage)(self.handle, &XrSwapchainImageReleaseInfo::default()) }
        };
        recorded?;
        released.to_result("xrReleaseSwapchainImage")
    }

    fn destroy(&mut self, xr: &LibOpenXr, dev: &Device) {
        for slot in self.slots.drain(..) {
            unsafe {
                let _ = dev.raw.wait_for_fences(&[slot.fence], true, u64::MAX);
                dev.raw.destroy_fence(slot.fence, None);
                dev.raw.destroy_command_pool(slot.pool, None);
            }
        }
        unsafe { (xr.xrDestroySwapchain)(self.handle) }.log_error("xrDestroySwapchain");
    }

    /// The layer of eye `i` (or the whole image of a one-layer chain) as a sub image.
    fn sub_image(&self, layer: u32) -> XrSwapchainSubImage {
        XrSwapchainSubImage { swapchain: self.handle, image_rect: XrRect2Di { offset: XrOffset2Di { x: 0, y: 0 },
            extent: XrExtent2Di { width: self.size[0] as i32, height: self.size[1] as i32 } }, image_array_index: layer }
    }
}

impl Slot {
    fn new(dev: &Device) -> Result<Slot, String> {
        unsafe {
            let pool = dev.raw.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(dev.family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None).map_err(|e| format!("XR command pool: {e:?}"))?;
            let cb = dev.raw.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool)
                .level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1)).map_err(|e| format!("XR command buffer: {e:?}"))?[0];
            let fence = dev.raw.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None).map_err(|e| format!("XR fence: {e:?}"))?;
            Ok(Slot { pool, cb, fence, keep: None })
        }
    }

    /// Records `body` between the barriers that take `dst` (an acquired XR image of `layers`
    /// layers, in COLOR_ATTACHMENT_OPTIMAL) to a transfer destination and back, and submits it
    /// with this slot's fence. Earlier submissions on the queue (the app's frame) are ordered
    /// before it.
    fn record(&mut self, dev: &Device, dst: vk::Image, layers: u32, body: impl FnOnce(&Device, vk::CommandBuffer)) -> Result<(), String> {
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(layers);
        let barrier = |old, new, src, dest| vk::ImageMemoryBarrier::default().image(dst).old_layout(old).new_layout(new)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .src_access_mask(src).dst_access_mask(dest).subresource_range(range);
        unsafe {
            dev.raw.wait_for_fences(&[self.fence], true, u64::MAX).map_err(|e| format!("XR copy fence: {e:?}"))?;
            self.keep = None;
            dev.raw.reset_fences(&[self.fence]).map_err(|e| format!("XR copy fence reset: {e:?}"))?;
            dev.raw.reset_command_buffer(self.cb, vk::CommandBufferResetFlags::empty()).map_err(|e| format!("XR copy reset: {e:?}"))?;
            dev.raw.begin_command_buffer(self.cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(|e| format!("XR copy begin: {e:?}"))?;
            dev.raw.cmd_pipeline_barrier(self.cb, vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::MEMORY_WRITE).dst_access_mask(vk::AccessFlags::TRANSFER_READ)], &[],
                &[barrier(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::COLOR_ATTACHMENT_WRITE, vk::AccessFlags::TRANSFER_WRITE)]);
            body(dev, self.cb);
            dev.raw.cmd_pipeline_barrier(self.cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, vk::DependencyFlags::empty(), &[], &[],
                &[barrier(vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE)]);
            dev.raw.end_command_buffer(self.cb).map_err(|e| format!("XR copy end: {e:?}"))?;
            let cbs = [self.cb];
            let _queue = lock(&dev.queue_lock);
            dev.raw.queue_submit(dev.queue, &[vk::SubmitInfo::default().command_buffers(&cbs)], self.fence).map_err(|e| format!("XR copy submit: {e:?}"))?;
        }
        Ok(())
    }
}

/// Copies a frame image (BGRA8 UNORM in TRANSFER_SRC_OPTIMAL, as the core hands it over) into
/// `dst` bit for bit: `layers` regions of `size` side by side in the frame go to layers
/// 0..layers (the eyes of a stereo frame; the whole frame for a one-layer chain). The frames
/// hold sRGB-encoded colour and the swapchain is sRGB. A swapchain in RGBA order goes through
/// `staging` (RGBA8 UNORM): a blit there only swaps the channels, then the copy.
fn copy_frame(dev: &Device, cb: vk::CommandBuffer, src: &Image, dst: vk::Image, size: [u32; 2], layers: u32, staging: Option<&Image>) {
    let sub = |layer| vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).base_array_layer(layer).layer_count(1);
    let copies: Vec<vk::ImageCopy> = (0..layers).map(|l| vk::ImageCopy::default().src_subresource(sub(0)).dst_subresource(sub(l))
        .src_offset(vk::Offset3D { x: (l * size[0]) as i32, y: 0, z: 0 }).extent(vk::Extent3D { width: size[0], height: size[1], depth: 1 })).collect();
    unsafe {
        let Some(staging) = staging else {
            dev.raw.cmd_copy_image(cb, src.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, dst, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &copies);
            return;
        };
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let barrier = |old, new, src_access, dst_access| vk::ImageMemoryBarrier::default().image(staging.image).old_layout(old).new_layout(new)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .src_access_mask(src_access).dst_access_mask(dst_access).subresource_range(range);
        dev.raw.cmd_pipeline_barrier(cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[],
            &[barrier(vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::TRANSFER_READ, vk::AccessFlags::TRANSFER_WRITE)]);
        let corner = vk::Offset3D { x: (size[0] * layers) as i32, y: size[1] as i32, z: 1 };
        let region = vk::ImageBlit::default().src_subresource(sub(0)).dst_subresource(sub(0))
            .src_offsets([vk::Offset3D::default(), corner]).dst_offsets([vk::Offset3D::default(), corner]);
        dev.raw.cmd_blit_image(cb, src.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, staging.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region], vk::Filter::NEAREST);
        dev.raw.cmd_pipeline_barrier(cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[],
            &[barrier(vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::TRANSFER_READ)]);
        dev.raw.cmd_copy_image(cb, staging.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, dst, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &copies);
    }
}

/// An eye's pose (in the local space) and field of view, as located for a frame.
#[derive(Clone, Copy)]
struct Eye { pose: XrPosef, fov: XrFovf }

/// The panel's pointer: the hand that points (the one that pressed last; the right one at
/// first), whether it is pressed, and where it is on the panel (window points).
#[derive(Default)]
struct Pointer { hand: usize, pressed: bool, at: Option<[f64; 2]>, inside: bool }

/// The old `CxOpenXrSession`, run by the XR thread: its swapchains are the panel's, the pointer
/// dot's and the stereo window's projection layer.
struct CxOpenXrSession {
    rt: Arc<Runtime>,
    dev: Arc<Device>,
    frames: Arc<Frames>,
    handle: XrSession,
    head_space: XrSpace,
    local_space: XrSpace,
    /// LOCAL: where the panel hangs (LOCAL_FLOOR is LOCAL lowered to the floor, so the panel is
    /// at the height of the head at the start)
    panel_space: XrSpace,
    passthrough: XrPassthroughFB,
    passthrough_layer: XrPassthroughLayerFB,
    depth_provider: XrEnvironmentDepthProviderMETA,
    depth_swap_chain: XrEnvironmentDepthSwapchainMETA,
    inputs: CxOpenXrInputs,
    active: bool,
    focused: bool,
    active_display_refresh_rate_hz: Option<f32>,
    panel: Chain,
    cursor: Chain,
    /// the projection layer: a layer per eye (made with the XR window)
    stereo: Option<Chain>,
    format: vk::Format,
    /// the swapchain format is RGBA ordered (the frames are BGRA): copies swizzle through staging
    blit: bool,
    pointer: Pointer,
    controllers: [XrController; 2],
    hands: [XrHand; 2],
    /// a panel frame was released into its chain (its layer may be shown), and its size
    panel_shown: Option<[u32; 2]>,
    /// the panel shows (always without an XR window; with one, the left thumbstick click toggles it)
    panel_visible: bool,
    cursor_ready: bool,
    /// the eyes the stereo frame in flight was asked for, and the eyes of the frame the
    /// projection layer shows
    stereo_asked: Option<[Eye; 2]>,
    stereo_shown: Option<[Eye; 2]>,
    stereo_asked_at: Instant,
    /// counters for the periodic timing line: display frames, stereo frames shown, frames
    /// whose stereo frame came too late, and the summed wait for stereo frames (ns)
    stats: [u64; 4],
    stats_at: Instant,
}

unsafe impl Send for CxOpenXrSession {}

impl CxOpenXrSession {
    fn update_active_display_refresh_rate(&mut self) -> Result<Option<f32>, String> {
        let Some(get_display_refresh_rate) = self.rt.xr.xrGetDisplayRefreshRateFB else {
            return Ok(self.active_display_refresh_rate_hz);
        };
        let mut refresh_rate_hz = 0.0f32;
        unsafe { (get_display_refresh_rate)(self.handle, &mut refresh_rate_hz) }.to_result("xrGetDisplayRefreshRateFB")?;
        self.active_display_refresh_rate_hz = (refresh_rate_hz.is_finite() && refresh_rate_hz > 0.0).then_some(refresh_rate_hz);
        Ok(self.active_display_refresh_rate_hz)
    }

    fn create_reference_space_with_fallback(xr: &LibOpenXr, session: XrSession, candidates: &[XrReferenceSpaceType]) -> Result<(XrSpace, XrReferenceSpaceType), String> {
        let mut errors = Vec::new();
        for &space_type in candidates {
            let create_info = XrReferenceSpaceCreateInfo { reference_space_type: space_type, ..Default::default() };
            let mut space = XrSpace(0);
            let result = unsafe { (xr.xrCreateReferenceSpace)(session, &create_info, &mut space) };
            if result == XrResult::SUCCESS {
                return Ok((space, space_type));
            }
            errors.push(format!("{space_type:?}: {result:?}"));
        }
        Err(format!("xrCreateReferenceSpace failed for all candidates: {}", errors.join(", ")))
    }

    /// The old `describe_primary_stereo_session`: the view configuration checks, the head (VIEW)
    /// space and the local space (LOCAL_FLOOR, else STAGE, else LOCAL).
    fn describe_primary_stereo_session(xr: &LibOpenXr, instance: XrInstance, system_id: XrSystemId, session: XrSession) -> Result<(XrSpace, XrSpace, XrReferenceSpaceType), String> {
        let configs = xr_array_fetch(XrViewConfigurationType::default(), |cap, len, buf| {
            unsafe { (xr.xrEnumerateViewConfigurations)(instance, system_id, cap, len, buf) }.to_result("xrEnumerateViewConfigurations")
        })?;
        if !configs.iter().any(|v| *v == XrViewConfigurationType::PRIMARY_STEREO) {
            return Err("Could not find PRIMARY STEREO viewconfiguration".to_string());
        }
        let mut config_props = XrViewConfigurationProperties::default();
        unsafe { (xr.xrGetViewConfigurationProperties)(instance, system_id, XrViewConfigurationType::PRIMARY_STEREO, &mut config_props) }.to_result("xrGetViewConfigurationProperties")?;

        let mut head_space = XrSpace(0);
        let head_space_info = XrReferenceSpaceCreateInfo { reference_space_type: XrReferenceSpaceType::VIEW, ..Default::default() };
        unsafe { (xr.xrCreateReferenceSpace)(session, &head_space_info, &mut head_space) }.to_result("xrCreateReferenceSpace")?;

        let (local_space, local_space_type) = Self::create_reference_space_with_fallback(xr, session,
            &[XrReferenceSpaceType::LOCAL_FLOOR, XrReferenceSpaceType::STAGE, XrReferenceSpaceType::LOCAL])?;
        Ok((head_space, local_space, local_space_type))
    }

    /// The old `create_passthrough_and_depth`: passthrough runs from the start (its layer is
    /// under every frame), and the environment depth provider and swapchain are made.
    fn create_passthrough_and_depth(xr: &LibOpenXr, session: XrSession, options: CxOpenXrOptions)
        -> Result<(XrPassthroughFB, XrPassthroughLayerFB, XrEnvironmentDepthProviderMETA, XrEnvironmentDepthSwapchainMETA), String> {
        let mut passthrough = XrPassthroughFB(0);
        let ptci = XrPassthroughCreateInfoFB { flags: XrPassthroughFlagsFB(0), ..Default::default() };
        unsafe { (xr.xrCreatePassthroughFB)(session, &ptci, &mut passthrough) }.to_result("xrCreatePassthroughFB")?;

        let plci = XrPassthroughLayerCreateInfoFB { passthrough, purpose: XrPassthroughLayerPurposeFB::RECONSTRUCTION, ..Default::default() };
        let mut passthrough_layer = XrPassthroughLayerFB(0);
        unsafe { (xr.xrCreatePassthroughLayerFB)(session, &plci, &mut passthrough_layer) }.to_result("xrCreatePassthroughLayerFB")?;
        unsafe { (xr.xrPassthroughStartFB)(passthrough) }.to_result("xrPassthroughStartFB")?;
        unsafe { (xr.xrPassthroughLayerResumeFB)(passthrough_layer) }.to_result("xrPassthroughLayerResumeFB")?;

        // Environment depth needs the USE_SCENE permission granted: without it the session
        // runs with passthrough and no depth (the old backend failed the session)
        let depth = (|| {
            let edpci = XrEnvironmentDepthProviderCreateInfoMETA { create_flags: XrEnvironmentDepthProviderCreateFlagsMETA(0), ..Default::default() };
            let mut depth_provider = XrEnvironmentDepthProviderMETA(0);
            unsafe { (xr.xrCreateEnvironmentDepthProviderMETA)(session, &edpci, &mut depth_provider) }.to_result("xrCreateEnvironmentDepthProviderMETA")?;

            let edhrsi = XrEnvironmentDepthHandRemovalSetInfoMETA { enabled: XrBool32::from_bool(options.remove_hands_from_depth), ..Default::default() };
            unsafe { (xr.xrSetEnvironmentDepthHandRemovalMETA)(depth_provider, &edhrsi) }.to_result("xrSetEnvironmentDepthHandRemovalMETA")?;

            let edsci = XrEnvironmentDepthSwapchainCreateInfoMETA { ty: XrStructureType::ENVIRONMENT_DEPTH_SWAPCHAIN_CREATE_INFO_META, next: ptr::null(), create_flags: XrEnvironmentDepthSwapchainCreateFlagsMETA(0) };
            let mut depth_swap_chain = XrEnvironmentDepthSwapchainMETA(0);
            unsafe { (xr.xrCreateEnvironmentDepthSwapchainMETA)(depth_provider, &edsci, &mut depth_swap_chain) }.to_result("xrCreateEnvironmentDepthSwapchainMETA")?;

            let mut edss = XrEnvironmentDepthSwapchainStateMETA { ty: XrStructureType::ENVIRONMENT_DEPTH_SWAPCHAIN_STATE_META, next: ptr::null_mut(), width: 0, height: 0 };
            unsafe { (xr.xrGetEnvironmentDepthSwapchainStateMETA)(depth_swap_chain, &mut edss) }.to_result("xrGetEnvironmentDepthSwapchainStateMETA")?;
            Ok::<_, String>((depth_provider, depth_swap_chain))
        })();
        let (depth_provider, depth_swap_chain) = depth.unwrap_or_else(|e| {
            xr_log!("no environment depth: {e}");
            (XrEnvironmentDepthProviderMETA(0), XrEnvironmentDepthSwapchainMETA(0))
        });
        Ok((passthrough, passthrough_layer, depth_provider, depth_swap_chain))
    }

    /// The old `create_session` / `create_session_vulkan`, on the HAL's device.
    fn create_session(rt: Arc<Runtime>, dev: Arc<Device>, frames: Arc<Frames>, options: CxOpenXrOptions) -> Result<Self, String> {
        let xr = &rt.xr;
        let binding = XrGraphicsBindingVulkanKHR { ty: XrStructureType::GRAPHICS_BINDING_VULKAN_KHR, next: ptr::null(), instance: dev.instance.handle().0 as VkInstance,
            physical_device: dev.physical.0 as VkPhysicalDevice, device: dev.raw.handle().0 as VkDevice, queue_family_index: dev.family, queue_index: 0 };
        let info = XrSessionCreateInfo { ty: XrStructureType::SESSION_CREATE_INFO, next: &binding as *const _ as *const c_void, create_flags: XrSessionCreateFlags(0), system_id: rt.system };
        let mut handle = XrSession(0);
        unsafe { (xr.xrCreateSession)(rt.instance, &info, &mut handle) }.to_result("xrCreateSession")?;
        let (head_space, local_space, local_space_type) = Self::describe_primary_stereo_session(xr, rt.instance, rt.system, handle)?;
        let mut panel_space = XrSpace(0);
        let panel_info = XrReferenceSpaceCreateInfo { reference_space_type: XrReferenceSpaceType::LOCAL, ..Default::default() };
        unsafe { (xr.xrCreateReferenceSpace)(handle, &panel_info, &mut panel_space) }.to_result("xrCreateReferenceSpace")?;

        let formats = xr_array_fetch(0i64, |cap, len, buf| unsafe { (xr.xrEnumerateSwapchainFormats)(handle, cap, len, buf) }.to_result("xrEnumerateSwapchainFormats"))?;
        // The frames are BGRA8 UNORM holding sRGB-encoded colour: copied bit for bit into an
        // sRGB image the compositor shows them as on a phone (`copy_frame`; Quest 3 offers
        // only the RGBA order).
        let format = vulkan::pick_vulkan_color_format(&formats, vk::Format::B8G8R8A8_SRGB)
            .ok_or_else(|| format!("OpenXR Vulkan found no supported 32-bit RGBA swapchain format; runtime={formats:?}"))?;
        let blit = matches!(format, vk::Format::R8G8B8A8_SRGB | vk::Format::R8G8B8A8_UNORM);
        let (passthrough, passthrough_layer, depth_provider, depth_swap_chain) = Self::create_passthrough_and_depth(xr, handle, options)?;
        let inputs = CxOpenXrInputs::new_inputs(xr, handle, rt.instance)?;
        xr_log!("session {handle:?}: local space {local_space_type:?}, swapchain {format:?}{}; passthrough running",
            if blit { " (swizzled through staging)" } else { "" });
        let panel = Chain::new(xr, &dev, handle, format, PANEL_PX, 1, XrSwapchainCreateFlags(0), blit)?;
        let cursor = Chain::new(xr, &dev, handle, format, [CURSOR_PX; 2], 1, XrSwapchainCreateFlags::STATIC_IMAGE, false)?;
        let mut session = Self { rt: rt.clone(), dev, frames, handle, head_space, local_space, panel_space, passthrough, passthrough_layer, depth_provider, depth_swap_chain,
            inputs, active: false, focused: false, active_display_refresh_rate_hz: None, panel, cursor, stereo: None, format, blit,
            pointer: Pointer { hand: 1, ..Default::default() }, controllers: Default::default(), hands: Default::default(),
            panel_shown: None, panel_visible: true, cursor_ready: false, stereo_asked: None, stereo_shown: None, stereo_asked_at: Instant::now(),
            stats: [0; 4], stats_at: Instant::now() };
        let _ = session.update_active_display_refresh_rate();
        Ok(session)
    }

    /// The old `begin_session`: performance levels, the thread hints, the refresh rate. On the
    /// HAL the app's frames are made on other threads; the XR thread renders (copies) and paces.
    fn begin_session(&mut self) {
        let xr = &self.rt.xr;
        let session_begin_info = XrSessionBeginInfo { ty: XrStructureType::SESSION_BEGIN_INFO, next: ptr::null(), primary_view_configuration_type: XrViewConfigurationType::PRIMARY_STEREO };
        let begin_result = unsafe { (xr.xrBeginSession)(self.handle, &session_begin_info) };
        if begin_result != XrResult::SUCCESS {
            xr_log!("OpenXR begin_session failed: {:?}", begin_result);
            return;
        }
        self.active = true;
        unsafe { (xr.xrPerfSettingsSetPerformanceLevelEXT)(self.handle, XrPerfSettingsDomainEXT::CPU, XrPerfSettingsLevelEXT::SUSTAINED_HIGH) }.log_error("xrPerfSettingsSetPerformanceLevelEXT CPU");
        unsafe { (xr.xrPerfSettingsSetPerformanceLevelEXT)(self.handle, XrPerfSettingsDomainEXT::GPU, XrPerfSettingsLevelEXT::SUSTAINED_HIGH) }.log_error("xrPerfSettingsSetPerformanceLevelEXT GPU");
        unsafe { (xr.xrSetAndroidApplicationThreadKHR)(self.handle, XrAndroidThreadTypeKHR::APPLICATION_MAIN, self.rt.app_thread) }.log_error("xrSetAndroidApplicationThreadKHR");
        unsafe { (xr.xrSetAndroidApplicationThreadKHR)(self.handle, XrAndroidThreadTypeKHR::RENDERER_MAIN, gettid() as u32) }.log_error("xrSetAndroidApplicationThreadKHR");
        if let Err(err) = self.update_active_display_refresh_rate() {
            xr_log!("OpenXR failed to query active display refresh rate: {err}");
        }
        // the session, not the Activity, decides when windows draw (the old loop kept running
        // while the Activity was paused)
        lock(&Shared::global().state).active = true;
    }

    fn end_session(&mut self) {
        xr_log!("OpenXR end_session handle={:?} active={}", self.handle, self.active);
        unsafe { (self.rt.xr.xrEndSession)(self.handle) }.log_error("xrEndSession");
        self.active = false;
    }

    /// The XR frame loop, for the life of the session.
    fn run(mut self) {
        loop {
            if !self.handle_events() { break; }
            if !self.active { std::thread::sleep(Duration::from_millis(10)); continue; }
            if let Err(e) = self.frame() {
                xr_log!("frame: {e}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        xr_log!("the session ended; the XR thread stops");
    }

    /// The old `openxr_handle_events`; `false` once the session is exiting or lost.
    fn handle_events(&mut self) -> bool {
        let rt = self.rt.clone();
        let xr = &rt.xr;
        loop {
            let mut event_buffer = XrEventDataBuffer { ty: XrStructureType::EVENT_DATA_BUFFER, next: ptr::null(), varying: [0; 4000] };
            if unsafe { (xr.xrPollEvent)(rt.instance, &mut event_buffer) } != XrResult::SUCCESS { return true; }
            match event_buffer.ty {
                XrStructureType::EVENT_DATA_SESSION_STATE_CHANGED => {
                    let edssc = unsafe { *(&event_buffer as *const _ as *const XrEventDataSessionStateChanged) };
                    xr_log!("session state {:?}", edssc.state);
                    match edssc.state {
                        XrSessionState::READY => self.begin_session(),
                        XrSessionState::STOPPING => self.end_session(),
                        XrSessionState::EXITING | XrSessionState::LOSS_PENDING => return false,
                        _ => (),
                    }
                    // the app is focused while it shows (VISIBLE or FOCUSED): a system dialog over
                    // it (VISIBLE) must not park the app, which the old backend never did either
                    let focused = edssc.state == XrSessionState::FOCUSED || edssc.state == XrSessionState::VISIBLE;
                    if focused != self.focused {
                        self.focused = focused;
                        let shared = Shared::global();
                        lock(&shared.state).focused = focused;
                        shared.emit(clock_ns(), |q, id| { q.focus(id, focused); q.lifecycle(if focused { lifecycle::FOCUS_GAINED } else { lifecycle::FOCUS_LOST }); });
                    }
                }
                XrStructureType::EVENT_DATA_DISPLAY_REFRESH_RATE_CHANGED_FB => {
                    let event = unsafe { *(&event_buffer as *const _ as *const XrEventDataDisplayRefreshRateChangedFB) };
                    if event.to_display_refresh_rate.is_finite() && event.to_display_refresh_rate > 0.0 {
                        self.active_display_refresh_rate_hz = Some(event.to_display_refresh_rate);
                        xr_log!("OpenXR display refresh changed: {:.1} Hz -> {:.1} Hz", event.from_display_refresh_rate, event.to_display_refresh_rate);
                    }
                }
                XrStructureType::EVENT_DATA_REFERENCE_SPACE_CHANGE_PENDING => {
                    // the old backend reset its depth (TSDF) store here; the app sees the new
                    // poses in the next frames
                    xr_log!("OpenXR reference space changed");
                }
                XrStructureType::EVENT_DATA_INSTANCE_LOSS_PENDING => { xr_log!("instance loss pending"); return false; }
                _ => {}
            }
        }
    }

    fn frame(&mut self) -> Result<(), String> {
        let rt = self.rt.clone();
        let xr = &rt.xr;
        let mut fi = XrFrameWaitInfo::default();
        let mut frame_state = XrFrameState::default();
        unsafe { (xr.xrWaitFrame)(self.handle, &mut fi, &mut frame_state) }.to_result("xrWaitFrame")?;
        let time = frame_state.predicted_display_time;
        // the old backend drew and submitted every frame whatever `should_render` said (the
        // runtime clears it while the session is only VISIBLE, which kept the panel and the
        // stereo view from ever showing)
        let render = true;
        if !frame_state.should_render.as_bool() && self.stats[0] == 0 { xr_log!("should_render is off (submitting anyway)"); }
        let period = frame_state.predicted_display_period.as_nanos();
        let shared = Shared::global();
        let hz = self.active_display_refresh_rate_hz.or_else(|| (period > 0).then(|| (1e9 / period as f64) as f32));
        if let Some(hz) = hz { lock(&shared.state).refresh = hz; }
        // the other windows' frames for this display frame (XrTime is CLOCK_MONOTONIC on
        // Android, the HAL clock)
        shared.frame(time.as_nanos() as u64);
        {
            let _queue = lock(&self.dev.queue_lock);
            unsafe { (xr.xrBeginFrame)(self.handle, &XrFrameBeginInfo::default()) }.to_result("xrBeginFrame")?;
        }
        let previous = self.controllers;
        match self.inputs.poll(xr, self.handle, self.local_space, time) {
            Some((controllers, hands)) => { self.controllers = controllers; self.hands = hands; }
            // not focused: nothing is held
            None => { self.controllers = Default::default(); self.hands = Default::default(); }
        }
        let xr_window = lock(&shared.registry).xr_window;
        if xr_window.is_some() {
            // a click of the left thumbstick shows and hides the panel over the stereo view (the
            // face and menu buttons stay the app's)
            let click = |c: &XrController| c.buttons & xr_controller::CLICK_THUMBSTICK != 0;
            if click(&self.controllers[0]) && !click(&previous[0]) { self.panel_visible = !self.panel_visible; }
        } else {
            self.panel_visible = true;
        }
        // the panel's plane in the local space: LOCAL_FLOOR is LOCAL lowered to the floor
        let panel_pose = {
            let mut loc = XrSpaceLocation::locate(xr, self.local_space, time, self.panel_space).pose;
            loc.position.z += PANEL_Z;
            loc.orientation = Default::default();
            loc.orientation.w = 1.;
            loc
        };
        let hit = if self.panel_visible { self.point(panel_pose) } else { self.leave_panel(); None };
        let panel_done = if render { self.update_panel()? } else { None };
        let stereo_done = match xr_window { Some((sid, id)) if render => self.update_stereo(sid, id, time, period)?, _ => None };
        if !self.cursor_ready && render { self.fill_cursor()?; }

        let quad = |chain: &Chain, size: [u32; 2], pose, metres: [f32; 2]| {
            let mut sub_image = chain.sub_image(0);
            sub_image.image_rect.extent = XrExtent2Di { width: size[0] as i32, height: size[1] as i32 };
            XrCompositionLayerQuad { ty: XrStructureType::COMPOSITION_LAYER_QUAD, next: ptr::null(), layer_flags: XrCompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA,
                space: self.local_space, eye_visibility: XrEyeVisibility::BOTH, sub_image, pose, size: XrExtent2Df { width: metres[0], height: metres[1] } }
        };
        let panel = self.panel_shown.filter(|_| self.panel_visible).map(|size| quad(&self.panel, size, panel_pose, PANEL_M));
        let cursor = hit.filter(|_| self.cursor_ready && panel.is_some()).map(|p| {
            let mut pose = panel_pose;
            pose.position.x = p[0]; pose.position.y = p[1]; pose.position.z += 0.003;
            quad(&self.cursor, [CURSOR_PX; 2], pose, [CURSOR_M; 2])
        });
        // the old end_frame: passthrough under a projection that blends with its alpha
        let comp_passthrough = XrCompositionLayerPassthroughFB { layer_handle: self.passthrough_layer, flags: XrCompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA, ..Default::default() };
        let proj_views = match (&self.stereo, self.stereo_shown) {
            (Some(chain), Some(eyes)) => Some([0, 1].map(|eye| XrCompositionLayerProjectionView { pose: eyes[eye].pose, fov: eyes[eye].fov, sub_image: chain.sub_image(eye as u32), ..Default::default() })),
            _ => None,
        };
        let comp_proj = proj_views.as_ref().map(|views| XrCompositionLayerProjection { space: self.local_space, view_count: 2, views: views as *const _,
            layer_flags: XrCompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA | XrCompositionLayerFlags::CORRECT_CHROMATIC_ABERRATION, ..Default::default() });
        let mut layers: Vec<*const XrCompositionLayerBaseHeader> = vec![&comp_passthrough as *const _ as *const XrCompositionLayerBaseHeader];
        if render {
            if let Some(l) = &comp_proj { layers.push(l as *const _ as *const XrCompositionLayerBaseHeader); }
            if let Some(l) = &panel { layers.push(l as *const _ as *const XrCompositionLayerBaseHeader); }
            if let Some(l) = &cursor { layers.push(l as *const _ as *const XrCompositionLayerBaseHeader); }
        }
        let fei = XrFrameEndInfo { display_time: time, environment_blend_mode: XrEnvironmentBlendMode::OPAQUE, layer_count: layers.len() as _, layers: layers.as_ptr(), ..Default::default() };
        let r = { let _queue = lock(&self.dev.queue_lock); unsafe { (xr.xrEndFrame)(self.handle, &fei) } };
        let shown_at = time.as_nanos() as u64;
        if let Some(done) = panel_done { done(shown_at); }
        if let Some(done) = stereo_done { done(shown_at); }
        self.stats[0] += 1;
        if self.stats_at.elapsed() >= Duration::from_secs(5) {
            let [frames, shown, late, wait] = self.stats;
            let secs = self.stats_at.elapsed().as_secs_f64();
            let hands = self.hands.map(|h| if h.flags & xr_hand::IN_VIEW != 0 { "tracked" } else { "-" });
            xr_log!("{frames} display frames in {secs:.1} s ({:.1} Hz): {shown} stereo frames shown, {late} late, {:.2} ms average wait; panel {}; hands {hands:?}; focused {}",
                frames as f64 / secs, wait as f64 / 1e6 / shown.max(1) as f64, if self.panel_shown.is_some() { "up" } else { "no frame yet" }, self.focused);
            self.stats = [0; 4];
            self.stats_at = Instant::now();
        }
        r.to_result("xrEndFrame")
    }

    /// Copies the newest panel frame into the panel's next image; the frame's `presented`.
    fn update_panel(&mut self) -> Result<Option<Box<dyn FnOnce(u64) + Send>>, String> {
        let Some((image, done)) = lock(&self.frames.panel).take() else { return Ok(None) };
        let size = [image.desc.w.min(PANEL_PX[0]), image.desc.h.min(PANEL_PX[1])];
        let src = image.clone();
        let staging = self.panel.staging.clone();
        self.panel.write(&self.rt.xr, &self.dev, Some(image), |dev, cb, dst| copy_frame(dev, cb, &src, dst, size, 1, staging.as_deref()))?;
        if self.panel_shown.is_none() { xr_log!("first frame on the panel ({}x{} px)", size[0], size[1]); }
        self.panel_shown = Some(size);
        Ok(Some(done))
    }

    /// The head and both eyes at `time` in the local space (the old `begin_frame`: the views
    /// located in the head space, then put in the local space through the head's pose).
    fn locate(&self, time: XrTime) -> Option<(XrPosef, [Eye; 2])> {
        let xr = &self.rt.xr;
        let local_from_head = XrSpaceLocation::locate(xr, self.local_space, time, self.head_space);
        let valid = XrSpaceLocationFlags::ORIENTATION_VALID | XrSpaceLocationFlags::POSITION_VALID;
        if !local_from_head.location_flags.contains(valid) { return None; }
        let projection_info = XrViewLocateInfo { view_configuration_type: XrViewConfigurationType::PRIMARY_STEREO, display_time: time, space: self.head_space, ..Default::default() };
        let mut view_state = XrViewState::default();
        let mut projections = [XrView::default(); 2];
        let mut num_views = 0;
        unsafe { (xr.xrLocateViews)(self.handle, &projection_info, &mut view_state, 2, &mut num_views, &mut projections as *mut _) }.to_result("xrLocateViews").ok()?;
        if num_views != 2 { return None; }
        let eyes = [0, 1].map(|eye| Eye { pose: XrPosef::multiply(&local_from_head.pose, &projections[eye].pose), fov: projections[eye].fov });
        Some((local_from_head.pose, eyes))
    }

    /// The stereo window: asks for its frame of this display frame (when none is in flight),
    /// waits for it until shortly before the display time, and copies a frame that came into
    /// the projection chain (an eye per layer) with the eyes it was drawn with.
    fn update_stereo(&mut self, sid: u32, id: Id, time: XrTime, period: i64) -> Result<Option<Box<dyn FnOnce(u64) + Send>>, String> {
        let shared = Shared::global();
        let eye = EYE_PX.get().copied().unwrap_or([1, 1]);
        if self.stereo.is_none() {
            self.stereo = Some(Chain::new(&self.rt.xr, &self.dev, self.handle, self.format, eye, 2, XrSwapchainCreateFlags(0), self.blit)?);
        }
        // Every display frame asks the XR window for a frame (the app's XR pass repaints with
        // each new XrFrame), one at a time; a frame that never came (its batch dropped) is
        // asked again after STEREO_REASK.
        if self.stereo_asked.is_some() && self.stereo_asked_at.elapsed() > STEREO_REASK { self.stereo_asked = None; }
        if self.stereo_asked.is_none() {
            let Some((head, eyes)) = self.locate(time) else {
                if self.stats[0] % 360 == 0 { xr_log!("the head is not tracked: no stereo frame asked"); }
                return Ok(None)
            };
            lock(&shared.state).pending.remove(&(sid, id));
            let fov = |f: &XrFovf| [f.angle_left, f.angle_right, f.angle_up, f.angle_down];
            let frame = XrFrame { head: pose(head), eyes: eyes.map(|e| XrEye { pose: pose(e.pose), fov: fov(&e.fov) }), controllers: self.controllers, hands: self.hands };
            let mut bytes = Vec::with_capacity(<XrFrame as crate::buffers::Wire>::SIZE);
            crate::buffers::Wire::write(&frame, &mut bytes);
            let t = time.as_nanos();
            shared.emit_to(sid, id, clock_ns(), |q, id| { q.xr_frame(id, t as u64, &bytes); q.frame_ready(id, (t + period) as u64); });
            self.stereo_asked = Some(eyes);
            self.stereo_asked_at = Instant::now();
        }
        // wait for the frame asked for (it is drawn with the eyes of this display frame)
        let started = Instant::now();
        // at most half a display period: a frame that is not there keeps the last one showing,
        // and the loop keeps the display rate (waiting to the display time halved it and worse)
        let deadline = Duration::from_nanos((time.as_nanos() - STEREO_MARGIN_NS - clock_ns() as i64).clamp(0, period / 2) as u64);
        let mut slot = lock(&self.frames.stereo);
        while slot.is_none() && started.elapsed() < deadline {
            slot = self.frames.arrived.wait_timeout(slot, deadline - started.elapsed()).unwrap_or_else(|e| e.into_inner()).0;
        }
        let Some((image, done)) = slot.take() else { self.stats[2] += 1; return Ok(None) };
        drop(slot);
        self.stats[1] += 1;
        self.stats[3] += started.elapsed().as_nanos() as u64;
        let chain = self.stereo.as_mut().expect("stereo chain");
        let size = [(image.desc.w / 2).min(chain.size[0]), image.desc.h.min(chain.size[1])];
        let src = image.clone();
        let staging = chain.staging.clone();
        chain.write(&self.rt.xr, &self.dev, Some(image), |dev, cb, dst| copy_frame(dev, cb, &src, dst, size, 2, staging.as_deref()))?;
        if self.stereo_shown.is_none() { xr_log!("first stereo frame ({}x{} px per eye)", size[0], size[1]); }
        self.stereo_shown = self.stereo_asked.take();
        Ok(Some(done))
    }

    /// The pointer ray of hand `h`: the controller's aim while it is tracked, else the tracked
    /// hand's aim (XR_FB_hand_tracking_aim); and whether it presses (the trigger, or an index
    /// pinch).
    fn ray(&self, h: usize) -> Option<(XrPose, bool)> {
        let c = &self.controllers[h];
        if c.buttons & xr_controller::ACTIVE != 0 { return Some((c.aim_pose, c.trigger >= TRIGGER_PRESS)); }
        let hand = &self.hands[h];
        (hand.flags & xr_hand::AIM_VALID != 0).then(|| (hand.aim_pose, hand.flags & xr_hand::PINCH_INDEX != 0))
    }

    /// The pointing hand's ray as mouse events on the panel; the hit point on the panel's plane
    /// (local space, metres) while the ray is over the panel.
    fn point(&mut self, panel: XrPosef) -> Option<[f32; 2]> {
        let rays = [self.ray(0), self.ray(1)];
        let pressed = rays.map(|r| r.is_some_and(|(_, down)| down));
        let p = &mut self.pointer;
        // a press of the other hand makes it the pointer (not mid-drag)
        if !p.pressed { if let Some(h) = (0..2).find(|h| pressed[*h] && *h != p.hand) { p.hand = h; } }
        let hand = p.hand;
        let hit = rays[hand].and_then(|(aim, _)| panel_hit(aim, panel));
        let points = [PANEL_PX[0] as f64 / PANEL_DPI as f64, PANEL_PX[1] as f64 / PANEL_DPI as f64];
        let at = hit.map(|[u, v, ..]| [u as f64 * points[0], v as f64 * points[1]]);
        let inside = at.is_some_and(|[x, y]| (0. ..=points[0]).contains(&x) && (0. ..=points[1]).contains(&y));
        let down = pressed[hand];
        let shared = Shared::global();
        let now = clock_ns();
        if let Some(pos @ [x, y]) = at.filter(|_| inside || p.pressed) {
            if p.at != Some(pos) { shared.emit(now, |q, id| q.mouse_move(id, x, y, 0., 0., 0)); }
            if down && !p.pressed { shared.emit(now, |q, id| q.mouse_down(id, x, y, 1, 0)); }
            if !down && p.pressed { shared.emit(now, |q, id| q.mouse_up(id, x, y, 1, 0)); }
            p.at = Some(pos);
            p.pressed = down;
        } else {
            self.leave_panel();
        }
        let stick = self.controllers[hand].stick;
        let p = &mut self.pointer;
        p.inside = inside;
        if let Some([x, y]) = p.at.filter(|_| inside) {
            let axis = |v: f32| if v.abs() < STICK_DEAD { 0. } else { (v * STICK_SCROLL) as f64 };
            let (dx, dy) = (axis(stick[0]), axis(-stick[1]));
            if dx != 0. || dy != 0. { shared.emit(now, |q, id| q.scroll(id, x, y, dx, dy, 0, true, 0)); }
        }
        hit.filter(|_| inside).map(|[_, _, px, py]| [px, py])
    }

    /// The pointer is off the panel (or the panel is hidden): a press ends, the hover leaves.
    fn leave_panel(&mut self) {
        let p = &mut self.pointer;
        let shared = Shared::global();
        let [x, y] = p.at.unwrap_or_default();
        if p.pressed { shared.emit(clock_ns(), |q, id| q.mouse_up(id, x, y, 1, 0)); }
        if p.inside || p.at.is_some() { shared.emit(clock_ns(), |q, id| q.mouse_leave(id, x, y)); }
        *p = Pointer { hand: p.hand, ..Default::default() };
    }

    /// The pointer dot's one (static) image: opaque white.
    fn fill_cursor(&mut self) -> Result<(), String> {
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        self.cursor.write(&self.rt.xr, &self.dev, None, |dev, cb, dst| unsafe {
            dev.raw.cmd_clear_color_image(cb, dst, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &vk::ClearColorValue { float32: [1., 1., 1., 1.] }, &[range]);
        })?;
        self.cursor_ready = true;
        Ok(())
    }
}

/// The old `destroy_session`.
impl Drop for CxOpenXrSession {
    fn drop(&mut self) {
        let rt = self.rt.clone();
        let xr = &rt.xr;
        let dev = self.dev.clone();
        if self.active {
            unsafe { (xr.xrEndSession)(self.handle) }.log_error("xrEndSession");
            self.active = false;
        }
        self.panel.destroy(xr, &dev);
        self.cursor.destroy(xr, &dev);
        if let Some(stereo) = &mut self.stereo { stereo.destroy(xr, &dev); }
        if self.depth_provider.0 != 0 {
            unsafe { (xr.xrStopEnvironmentDepthProviderMETA)(self.depth_provider) }.log_error("xrStopEnvironmentDepthProviderMETA");
            unsafe { (xr.xrDestroyEnvironmentDepthProviderMETA)(self.depth_provider) }.log_error("xrDestroyEnvironmentDepthProviderMETA");
        }
        unsafe { (xr.xrPassthroughPauseFB)(self.passthrough) }.log_error("xrPassthroughPauseFB");
        unsafe { (xr.xrDestroyPassthroughFB)(self.passthrough) }.log_error("xrDestroyPassthroughFB");
        if self.depth_swap_chain.0 != 0 { unsafe { (xr.xrDestroyEnvironmentDepthSwapchainMETA)(self.depth_swap_chain) }.log_error("xrDestroyEnvironmentDepthSwapchainMETA"); }
        unsafe { (xr.xrDestroySpace)(self.head_space) }.log_error("xrDestroySpace");
        unsafe { (xr.xrDestroySpace)(self.local_space) }.log_error("xrDestroySpace");
        unsafe { (xr.xrDestroySpace)(self.panel_space) }.log_error("xrDestroySpace");
        // the action spaces and hand trackers belong to the session: they go first
        self.inputs.destroy_input(xr);
        unsafe { (xr.xrDestroySession)(self.handle) }.log_error("xrDestroySession");
    }
}

fn pose(p: XrPosef) -> XrPose {
    XrPose { orientation: [p.orientation.x, p.orientation.y, p.orientation.z, p.orientation.w], position: [p.position.x, p.position.y, p.position.z] }
}

/// Where a ray from `pose` (along its -Z) meets the panel at `panel` (facing +Z): (u, v) across
/// it from its top-left corner (0..1 on it) and the point (x, y) on its plane; `None` when it
/// points away.
fn panel_hit(pose: XrPose, panel: XrPosef) -> Option<[f32; 4]> {
    let [qx, qy, qz, qw] = pose.orientation;
    let o = pose.position;
    // v' = v + w t + q x t with t = 2 (q x v), v = (0, 0, -1)
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let qv = [qx, qy, qz];
    let t = cross(qv, [0., 0., -1.]).map(|c| 2. * c);
    let c = cross(qv, t);
    let d = [qw * t[0] + c[0], qw * t[1] + c[1], -1. + qw * t[2] + c[2]];
    if d[2] > -1e-4 { return None; }
    let c = panel.position;
    let s = (c.z - o[2]) / d[2];
    if s < 0. { return None; }
    let (x, y) = (o[0] + s * d[0], o[1] + s * d[1]);
    Some([(x - c.x + PANEL_M[0] / 2.) / PANEL_M[0], (c.y + PANEL_M[1] / 2. - y) / PANEL_M[1], x, y])
}
