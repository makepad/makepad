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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
/// The hands stay in the environment depth image (`remove_hands_from_depth` off): occlusion
/// needs them there (with removal the runtime fills the hand with the room behind it and only
/// the forearm hides virtual content); the room field masks them out with the tracked joints.
const XR_OPTIONS: CxOpenXrOptions = CxOpenXrOptions { buffer_scale: 1.0, multisamples: 1, remove_hands_from_depth: false, fixed_foveation_level: 3 };

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
/// The eyes the XR window's frame being encoded was drawn for when they are not its XrFrame's
/// (`HalCmd::XrFrameEyes`: a streamed picture's render eyes). They go with that frame: its
/// presenter's `scheduled` moves them to `SCHEDULED_EYES`, its `submit` hands them on with the
/// image. (One global taken when a frame was shown gave a frame the eyes of the next one, which
/// the render thread had already encoded: the picture shown a frame off its pose, a swim.)
/// With them the depth of what the frame shows (metres; 0 none: no depth layer).
static FRAME_EYES: Mutex<Option<([Eye; 2], f32)>> = Mutex::new(None);
/// The scheduled XR window frames' eyes, oldest first (frames complete in order).
static SCHEDULED_EYES: Mutex<std::collections::VecDeque<Option<([Eye; 2], f32)>>> = Mutex::new(std::collections::VecDeque::new());

/// The XR window's next frame shows with `eyes` (the projection views' poses and fields).
pub(super) fn frame_eyes(eyes: [XrEye; 2], focus_m: f32) {
    let eye = |e: &XrEye| Eye {
        pose: XrPosef { orientation: makepad_math::Quat { x: e.pose.orientation[0], y: e.pose.orientation[1], z: e.pose.orientation[2], w: e.pose.orientation[3] },
            position: makepad_math::Vec3f { x: e.pose.position[0], y: e.pose.position[1], z: e.pose.position[2] } },
        fov: XrFovf { angle_left: e.fov[0], angle_right: e.fov[1], angle_up: e.fov[2], angle_down: e.fov[3] },
    };
    *lock(&FRAME_EYES) = Some(([eye(&eyes[0]), eye(&eyes[1])], if focus_m.is_finite() { focus_m.max(0.0) } else { 0.0 }));
}

/// How far past its display time the XR window's XrFrames are located (`HalCmd::XrPoseLead`).
/// When the stereo window's frame in flight was scheduled (its CPU work done, handed to the GPU)
/// and when it arrived (its GPU work done): the frame loop's per-frame timing.
static STEREO_SCHEDULED_NS: AtomicU64 = AtomicU64::new(0);
static STEREO_ARRIVED_NS: AtomicU64 = AtomicU64::new(0);
/// The render thread's time in acquiring and waiting for the projection chain's images (summed
/// ns, count): the frame-loop stats.
static TARGET_WAIT_NS: AtomicU64 = AtomicU64::new(0);
static TARGET_WAITS: AtomicU64 = AtomicU64::new(0);
static POSE_LEAD_NS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// The app's frames write linear colour (`hal_xr_linear_frames`), and whether the projection
/// chains are made for it now.
static LINEAR_WANTED: AtomicBool = AtomicBool::new(false);
static LINEAR_ACTIVE: AtomicBool = AtomicBool::new(false);
pub fn linear_frames(on: bool) { LINEAR_WANTED.store(on, Ordering::Release); }
/// The projection chains' fixed foveation level (0 none .. 3 high; `hal_xr_foveation`), applied
/// by the XR thread when it changes.
static FOVEATION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub fn foveation(level: u32) { FOVEATION.store(level.min(3), Ordering::Release); }
/// Hands and controllers tracked at once (XR_META_simultaneous_hands_and_controllers; on at
/// start, `hal_xr_hands_and_controllers`). The runtime then shares the cameras with the
/// controllers: its hand tracker ran at 25 Hz (`muxMode=handsAndControllers`), against its
/// high-frequency hands mode without it.
static HANDS_AND_CONTROLLERS: AtomicBool = AtomicBool::new(true);
pub fn hands_and_controllers(on: bool) { HANDS_AND_CONTROLLERS.store(on, Ordering::Release); }
/// The environment depth provider runs (`hal_xr_environment_depth`; on at start).
static ENVIRONMENT_DEPTH: AtomicBool = AtomicBool::new(true);
pub fn environment_depth(on: bool) { ENVIRONMENT_DEPTH.store(on, Ordering::Release); }
/// The stereo view paused (`hal_xr_stereo_paused`): no frame asked, none shown.
static STEREO_PAUSED: AtomicBool = AtomicBool::new(false);
pub fn stereo_paused(on: bool) { STEREO_PAUSED.store(on, Ordering::Release); }

/// The panel's rule and placement (`hal_xr_panel`): shown (0 default, 1 always, 2 never),
/// distance, width (m), lazily following the head.
static PANEL_LAYOUT: Mutex<(u32, f32, f32, bool)> = Mutex::new((0, -PANEL_Z, PANEL_M[0], false));
pub fn panel(show: u32, distance_m: f32, width_m: f32, follow: bool) {
    *lock(&PANEL_LAYOUT) = (show, distance_m.clamp(0.3, 5.0), width_m.clamp(0.1, 4.0), follow);
}

/// A lazily following panel: where it hangs (local space, its yaw) and whether it is moving to
/// the view's centre.
#[derive(Clone, Copy, Default)]
struct PanelFollow { pos: [f32; 3], yaw: f32, moving: bool }
/// The depth images are copied back to the app (`hal_xr_depth_readback`; off at start).
pub(super) static DEPTH_READBACK: AtomicBool = AtomicBool::new(false);
pub fn depth_readback(on: bool) { DEPTH_READBACK.store(on, Ordering::Release); }
/// The eye buffers' size in percent of `EYE_PX` (the start size): now, and the range the
/// dynamic scale moves in (`hal_xr_eye_scale`; min = max: fixed). The XR thread steps it on the
/// runtime's measured app GPU frame time ([`EyeScale`]).
static EYE_PERCENT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(100);
static EYE_PERCENT_RANGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new((100 << 32) | 100);
pub fn eye_scale(min_percent: u32, max_percent: u32) {
    let max = max_percent.clamp(40, 100);
    let min = min_percent.clamp(40, max);
    EYE_PERCENT_RANGE.store(((min as u64) << 32) | max as u64, Ordering::Release);
}
/// The eye buffers' size now (percent of the start size) and the app GPU frame time the
/// dynamic scale last measured (µs, 0: none).
static EYE_GPU_US: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub fn eye_scale_now() -> (u32, u32) { (EYE_PERCENT.load(Ordering::Acquire), EYE_GPU_US.load(Ordering::Acquire)) }
/// The eye's pixels at `percent` of the start size (16-pixel multiples: the GPU's tiles).
fn eye_at(percent: u32) -> [u32; 2] {
    let base = EYE_PX.get().copied().unwrap_or([1, 1]);
    if percent >= 100 { return base; }
    base.map(|v| (((v * percent / 100) + 8) / 16 * 16).max(16))
}
pub fn linear_frames_active() -> bool { LINEAR_ACTIVE.load(Ordering::Acquire) }

/// The two projection chains (a layer per eye) and their render targets: `linear` sRGB images
/// without the mutable format (rendered through their own format: the shaders write linear
/// colour, the GPU keeps their framebuffer compression), else `format` with UNORM views.
#[allow(clippy::too_many_arguments)]
fn stereo_chains(xr: &LibOpenXr, dev: &Arc<Device>, session: XrSession, format: vk::Format, eye: [u32; 2], blit: bool, linear: bool) -> Result<(Chain, Chain, Targets), String> {
    let (format, usage, swizzle, target) = if linear {
        (vk::Format::R8G8B8A8_SRGB, Some(XrSwapchainUsageFlags::COLOR_ATTACHMENT | XrSwapchainUsageFlags::TRANSFER_DST), true, crate::format::RGBA8_SRGB)
    } else {
        (format, None, blit, if blit { crate::format::RGBA8 } else { crate::format::BGRA8 })
    };
    // foveated where the GPU takes density maps and the runtime offers foveation profiles
    let foveated = dev.fdm && xr.xrUpdateSwapchainFB.is_some() && xr.xrCreateFoveationProfileFB.is_some();
    let a = Chain::new_with(xr, dev, session, format, eye, 2, XrSwapchainCreateFlags(0), swizzle, usage, foveated)?;
    let b = Chain::new_with(xr, dev, session, format, eye, 2, XrSwapchainCreateFlags(0), false, usage, foveated)?;
    let targets = Targets::new(dev, &[&a, &b], target)?;
    log(&format!("projection chains: {format:?}{}", if linear { ", not mutable: linear frames" } else { ", mutable (UNORM views)" }));
    Ok((a, b, targets))
}

/// How many display frames past the newest one the uplink's poses are located for
/// (`hal_xr_pose_frames_ahead`; 0: from the pose lead instead).
static POSE_FRAMES_AHEAD: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
pub fn pose_frames_ahead(frames: u32) { POSE_FRAMES_AHEAD.store(frames.min(30) as i64, Ordering::Release); }

pub(super) fn pose_lead(lead_ns: u64) {
    POSE_LEAD_NS.store(lead_ns.min(500_000_000) as i64, Ordering::Release);
}

/// Controller buzzes asked for (`HalCmd::XrHaptic`: hand, amplitude, duration µs), played by the
/// XR thread after its next action sync.
static HAPTICS: std::sync::Mutex<Vec<(u32, f32, u32)>> = std::sync::Mutex::new(Vec::new());

pub(super) fn haptic(hand: u32, amplitude: f32, duration_us: u32) {
    let mut q = lock(&HAPTICS);
    if q.len() < 16 {
        q.push((hand.min(1), amplitude, duration_us.min(1_000_000)));
    }
}

/// The refresh rate the XR window asked for (mHz, 0: the runtime's default; `HalCmd::XrRefreshRate`),
/// requested by the XR thread when it differs from the one it requested last.
static REFRESH_MHZ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub(super) fn refresh_rate(millihertz: u32) {
    REFRESH_MHZ.store(millihertz, Ordering::Release);
}

/// The newest uplink pose (`hal_xr_uplink_next`): its time and `XrFrame`, and the time last
/// taken; the tracking thread's newest input; when the uplink was last asked for (ns); the
/// display period (ns).
static UPLINK: Mutex<(u64, Vec<u8>, u64)> = Mutex::new((0, Vec::new(), 0));
static UPLINK_NEW: std::sync::Condvar = std::sync::Condvar::new();
static TRACKED_INPUT: Mutex<Option<([XrController; 2], [XrHand; 2])>> = Mutex::new(None);
static UPLINK_ASKED_NS: AtomicU64 = AtomicU64::new(0);
static PERIOD_NS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(11_111_111);
/// The predicted display time of the XR thread's newest display frame (it wakes the tracking
/// thread).
static DISPLAY_FRAME: Mutex<u64> = Mutex::new(0);
static DISPLAY_FRAME_NEW: std::sync::Condvar = std::sync::Condvar::new();

fn uplink_wanted() -> bool { clock_ns().saturating_sub(UPLINK_ASKED_NS.load(Ordering::Acquire)) < 1_000_000_000 }

pub fn uplink_next(timeout_ms: u32, out: &mut [u8]) -> u64 {
    UPLINK_ASKED_NS.store(clock_ns(), Ordering::Release);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
    let mut u = lock(&UPLINK);
    loop {
        if u.0 != 0 && u.0 != u.2 && out.len() >= u.1.len() {
            out[..u.1.len()].copy_from_slice(&u.1);
            u.2 = u.0;
            return u.0;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() { return 0; }
        u = UPLINK_NEW.wait_timeout(u, left).unwrap_or_else(|e| e.into_inner()).0;
    }
}

pub(super) fn active() -> bool { ACTIVE.load(Ordering::Acquire) }

/// One eye's pixels while an XR session runs (the XR window is two of them side by side).
pub(super) fn eye_px() -> Option<[u32; 2]> { EYE_PX.get().filter(|_| active()).map(|_| eye_at(EYE_PERCENT.load(Ordering::Acquire))) }

/// XR lines go straight to logcat (tag `hal-xr`), live, whatever stderr is redirected to.
pub(crate) fn log(text: &str) {
    extern "C" { fn __android_log_write(prio: i32, tag: *const std::ffi::c_char, text: *const std::ffi::c_char) -> i32; }
    if let Ok(t) = std::ffi::CString::new(text.replace('\0', " ")) { unsafe { __android_log_write(4, c"hal-xr".as_ptr(), t.as_ptr()) }; }
}
macro_rules! xr_log { ($($t:tt)*) => { log(&format!($($t)*)) } }
mod depth;
mod track_log;

/// The tracking log from absolute index `cursor` (`hal_xr_track_log`).
pub fn track_log(cursor: u64, out: &mut [u8]) -> (u64, usize) { track_log::read(cursor, out) }
/// Plays a recorded tracking log instead of the runtime's poses (`hal_xr_replay`; empty: off).
pub fn replay(data: &[u8]) -> bool { track_log::set_replay(data, clock_ns()) }
/// One tracking log entry's size in bytes.
pub fn track_entry_size() -> usize { track_log::entry_size() }

/// `window_kind::XR` window `id` of session `sid` is made: it is the stereo view, sized by the
/// eyes (one pixel per point). `false` without an XR session.
pub(super) fn create_window(sid: u32, id: Id) -> bool {
    let Some(eye) = eye_px() else { return false };
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
        // (once: everything the runtime offers, for choosing what the compositor could do for us)
        xr_log!("runtime extensions ({}): {}", exts.len(), exts.iter().map(|e| xr_string_zero_terminated(&e.extension_name).trim_end_matches(char::from(0)).to_string()).collect::<Vec<_>>().join(" "));

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
        // the runtime's app GPU frame time (the dynamic eye scale)
        if has_extension("XR_META_performance_metrics\0") { exts_needed.push("XR_META_performance_metrics\0"); }
        // the projection layer's depth (positional reprojection)
        if has_extension("XR_KHR_composition_layer_depth\0") { exts_needed.push("XR_KHR_composition_layer_depth\0"); }
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
            // (debug `debug.xrremote.eyescale` N at start: the eye buffers at N percent of the
            // recommended size, the GPU's shading against sharpness)
            let percent = system_property("debug.xrremote.eyescale").and_then(|v| v.trim().parse::<u32>().ok()).filter(|p| (40..=150).contains(p)).unwrap_or(100);
            let scale = |v: u32| ((v as f32) * XR_OPTIONS.buffer_scale * percent as f32 / 100.0).max(1.0) as u32;
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
type Pending = (Arc<Image>, Box<dyn FnOnce(u64) + Send>, Option<([Eye; 2], f32)>);
/// The newest panel frame and the stereo window's frame in flight; `arrived` wakes the XR
/// thread waiting for a stereo frame.
#[derive(Default)]
/// `rgba`: the swapchains are RGBA ordered, so the panel's and the stereo window's targets are
/// made RGBA8 and copy into them as they are (a swizzling blit cost ~16 ms of CPU per frame on
/// Quest 3 to record).
/// `stereo_targets`: the projection chain's images the app renders into ([`Targets`]).
struct Frames { panel: Mutex<Option<Pending>>, stereo: Mutex<Option<Pending>>, arrived: Condvar, rgba: AtomicBool, stereo_targets: Mutex<Option<Targets>> }

/// A chain's images as the app's render targets (`XrSurfaces::target`): the render thread
/// acquires one per frame (acquire + wait) and renders the frame straight into it; the XR
/// thread releases them in acquire order when it takes a frame ([`Targets::release_through`]),
/// so the projection layer shows the newest frame with no copy. Every acquire, wait and release
/// of the chain goes through this lock. OpenXR lets one image be waited at a time: a frame's
/// image is acquired only when none is held (a second wait failed with CALL_ORDER_INVALID, the
/// chain's waited image fell one behind the rendered one, and the compositor showed images
/// still being drawn: torn, a frame late, with the newer frame's eyes).
/// `submitted`: the held image's frame is finished and was handed to the XR thread (a newer
/// frame may have replaced it before the XR thread took it).
/// Two chains: frames alternate between them. With one chain of three images a frame's image
/// was free only ~3-8 ms after the frame before was ended (the compositor still held the other
/// two), and every frame waited for it; the other chain's next image is free at once.
/// `images` are both chains' (`owner`: the chain of each), `acquired` holds their indices.
/// A pair of projection chains of one eye size and their render targets, kept while another
/// size is in use (`CxOpenXrSession::chain_spare`), and since when it is unused.
struct ChainSet { size: [u32; 2], a: Chain, b: Chain, targets: Targets, unused_since: Instant }
unsafe impl Send for ChainSet {}

impl ChainSet {
    fn destroy(&mut self, xr: &LibOpenXr, dev: &Arc<Device>) {
        self.a.destroy(xr, dev);
        self.b.destroy(xr, dev);
    }
}

/// A session handle moved to the chain builder (creating swapchains needs no external sync).
struct SendSession(XrSession);
unsafe impl Send for SendSession {}

struct Targets { handles: Vec<XrSwapchain>, images: Vec<Arc<Image>>, owner: Vec<usize>, base: Vec<usize>, next: usize, acquired: std::collections::VecDeque<usize>, submitted: bool }

impl Targets {
    /// The chain's images wrapped as `layers`-layer render targets of `size` in `format` (RGBA8 or
    /// BGRA8: UNORM views of the sRGB images, mutable format; RGBA8_SRGB: the images' own format,
    /// linear frames).
    fn new(dev: &Arc<Device>, chains: &[&Chain], format: u32) -> Result<Targets, String> {
        let (mut images, mut owner, mut base, mut handles) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (c, chain) in chains.iter().enumerate() {
            let desc = crate::session::TextureObj { format, w: chain.size[0], h: chain.size[1],
                depth: chain.layers, mips: 1, usage: crate::usage::RENDER_TARGET | crate::usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None };
            base.push(images.len());
            handles.push(chain.handle);
            for image in &chain.images {
                images.push(unsafe { Image::borrowed(dev.clone(), desc.clone(), *image, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL) }?);
                owner.push(c);
            }
        }
        Ok(Targets { handles, images, owner, base, next: 0, acquired: Default::default(), submitted: false })
    }

    /// The chain of an image.
    fn chain_of(&self, image: &Arc<Image>) -> Option<XrSwapchain> {
        self.images.iter().position(|i| Arc::ptr_eq(i, image)).map(|i| self.handles[self.owner[i]])
    }

    /// The next image for a frame of `size`, acquired and waited for; `None` when every image
    /// but one is held, or the chain is another size (the eye scale changed: the frame renders
    /// into the window target, the XR thread remakes the chains at its size and copies it in).
    fn acquire(&mut self, xr: &LibOpenXr, size: [u32; 2]) -> Option<Arc<Image>> {
        let first = self.images.first()?;
        if first.desc.w != size[0] || first.desc.h != size[1] { return None; }
        if !self.acquired.is_empty() { crate::profile::count("xr target: an image held (frame copied)"); return None; }
        let index = self.take_image(xr)?;
        self.acquired.push_back(index);
        self.submitted = false;
        let image = self.images.get(index)?.clone();
        image.set_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        Some(image)
    }

    /// Acquires the next chain's next image and waits for it: its index in `images`.
    fn take_image(&mut self, xr: &LibOpenXr) -> Option<usize> {
        let c = self.next;
        self.next = (c + 1) % self.handles.len();
        let handle = self.handles[c];
        let mut index = 0;
        let started = clock_ns();
        unsafe { (xr.xrAcquireSwapchainImage)(handle, &XrSwapchainImageAcquireInfo::default(), &mut index) }.to_result("xrAcquireSwapchainImage")
            .map_err(|e| xr_log!("{e}")).ok()?;
        let wait_info = XrSwapchainImageWaitInfo { timeout: XrDuration(1000000000), ..Default::default() };
        loop {
            let r = unsafe { (xr.xrWaitSwapchainImage)(handle, &wait_info) };
            if r == XrResult::TIMEOUT_EXPIRED { xr_log!("retry xrWaitSwapchainImage"); continue; }
            // (acquired and not waited: released all the same, so the chain stays in step)
            if let Err(e) = r.to_result("xrWaitSwapchainImage") { xr_log!("{e}"); }
            break;
        }
        TARGET_WAIT_NS.fetch_add(clock_ns() - started, Ordering::Relaxed);
        TARGET_WAITS.fetch_add(1, Ordering::Relaxed);
        Some(self.base[c] + index as usize)
    }

    /// Releases every acquired image up to and including `image` (oldest first: a release
    /// releases the oldest), so the chain shows `image`; `None` when `image` is none of them.
    fn release_through(&mut self, xr: &LibOpenXr, image: &Arc<Image>) -> Option<Result<(), String>> {
        let at = self.acquired.iter().position(|i| Arc::ptr_eq(&self.images[*i], image))?;
        self.submitted = false;
        for _ in 0..=at {
            let Some(i) = self.acquired.pop_front() else { break };
            if let Err(e) = unsafe { (xr.xrReleaseSwapchainImage)(self.handles[self.owner[i]], &XrSwapchainImageReleaseInfo::default()) }.to_result("xrReleaseSwapchainImage") {
                return Some(Err(e));
            }
        }
        Some(Ok(()))
    }
}

struct Presenter { frames: Arc<Frames>, stereo: bool }
impl FramePresenter for Presenter {
    fn submit(&self, _: u32, _: Id, image: Arc<Image>, presented: Box<dyn FnOnce(u64) + Send>) {
        let slot = if self.stereo { &self.frames.stereo } else { &self.frames.panel };
        let eyes = if self.stereo { lock(&SCHEDULED_EYES).pop_front().flatten() } else { None };
        if self.stereo {
            if let Some(t) = lock(&self.frames.stereo_targets).as_mut() {
                if t.acquired.back().is_some_and(|i| Arc::ptr_eq(&t.images[*i], &image)) { t.submitted = true; }
            }
        }
        if self.stereo { STEREO_ARRIVED_NS.store(clock_ns(), Ordering::Release); }
        let replaced = lock(slot).replace((image, presented, eyes));
        self.frames.arrived.notify_all();
        // a frame the XR thread never copied: it is over all the same (the app's frames in
        // flight are counted down)
        if let Some((_, done, _)) = replaced { done(clock_ns()); }
    }
    fn scheduled(&self, _: u32, _: Id) {
        if self.stereo {
            STEREO_SCHEDULED_NS.store(clock_ns(), Ordering::Release);
            lock(&SCHEDULED_EYES).push_back(lock(&FRAME_EYES).take());
        }
    }
    /// The runtime waits for the work submitted before an image's release, and the panel's copy
    /// is queued behind the frame: a frame is taken at its submission (waiting for the GPU to
    /// finish it put the GPU's time into the frame's deadline: frames late with the GPU idle)
    fn at_queue(&self) -> bool { true }
    fn dropped(&self, _: u32, _: Id) {
        if self.stereo { lock(&SCHEDULED_EYES).pop_front(); }
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
    fn target(&mut self, session: u32, window: Id, size: [u32; 2]) -> Option<Arc<Image>> {
        if lock(&Shared::global().registry).xr_window != Some((session, window)) { return None; }
        // with the previous frame's image still held (the XR thread releases it when it takes
        // that frame) this frame renders aside and is copied; it never waits
        lock(&self.frames.stereo_targets).as_mut()?.acquire(&self.rt.xr, size)
    }
    fn is_target(&self, image: &Arc<Image>) -> bool {
        lock(&self.frames.stereo_targets).as_ref().is_some_and(|t| t.images.iter().any(|i| Arc::ptr_eq(i, image)))
    }
    fn window_format(&self, session: u32, window: Id) -> u32 {
        let r = lock(&Shared::global().registry);
        let ours = r.xr_window == Some((session, window)) || (session == HOST && r.host == Some(window));
        if ours && self.frames.rgba.load(Ordering::Acquire) { crate::format::RGBA8 } else { crate::format::BGRA8 }
    }
    fn state(&mut self, session: u32, window: Id, _: [u32; 2]) -> SurfaceState {
        let r = lock(&Shared::global().registry);
        if r.xr_window == Some((session, window)) || (session == HOST && r.host == Some(window)) { SurfaceState::Managed } else { SurfaceState::Lost }
    }
}

/// The copy of a frame into one image of a chain: its command buffer and fence, and the frame
/// image read until that fence.
/// `lock_ns`, `submit_ns`: summed waits for the queue lock and vkQueueSubmit times.
struct Slot { pool: vk::CommandPool, cb: vk::CommandBuffer, fence: vk::Fence, keep: Option<Arc<Image>>, lock_ns: u64, submit_ns: u64, steps: [u64; 4] }

/// The projection layer's depth (XR_KHR_composition_layer_depth): a two-layer D16 swapchain
/// the size of the colour one, each image cleared to the depth of what the frame shows (its
/// focus: the compositor corrects the head's movement since the picture's render pose at that
/// depth). An image is cleared again only when its depth changes.
struct DepthChain { handle: XrSwapchain, images: Vec<vk::Image>, slots: Vec<(vk::CommandPool, vk::CommandBuffer, vk::Fence)>, cleared: Vec<f32>, size: [u32; 2] }

/// The depth chain's projection: Vulkan's 0..1 depth with these planes (metres).
const DEPTH_NEAR_M: f32 = 0.1;
const DEPTH_FAR_M: f32 = 100.0;

impl DepthChain {
    fn new(xr: &LibOpenXr, dev: &Device, session: XrSession, size: [u32; 2]) -> Result<DepthChain, String> {
        let usage = XrSwapchainUsageFlags::DEPTH_STENCIL_ATTACHMENT | XrSwapchainUsageFlags::TRANSFER_DST;
        let (handle, images) = vulkan::create_swapchain_for(xr, session, vk::Format::D16_UNORM, size[0], size[1], 2, XrSwapchainCreateFlags(0), usage)?;
        let mut slots = Vec::new();
        for _ in &images {
            unsafe {
                let pool = dev.raw.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(dev.family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None).map_err(|e| format!("XR depth layer pool: {e:?}"))?;
                let cb = dev.raw.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1)).map_err(|e| format!("XR depth layer command buffer: {e:?}"))?[0];
                let fence = dev.raw.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None).map_err(|e| format!("XR depth layer fence: {e:?}"))?;
                slots.push((pool, cb, fence));
            }
        }
        let cleared = vec![f32::NAN; images.len()];
        xr_log!("projection depth layer: {}x{} D16, {} images", size[0], size[1], images.len());
        Ok(DepthChain { handle, images, slots, cleared, size })
    }

    /// The depth value of `metres` along the view.
    fn depth_of(metres: f32) -> f32 {
        let z = metres.clamp(DEPTH_NEAR_M * 1.01, DEPTH_FAR_M * 0.99);
        DEPTH_FAR_M * (z - DEPTH_NEAR_M) / (z * (DEPTH_FAR_M - DEPTH_NEAR_M))
    }

    /// The next image, holding `metres` (cleared when it held another depth), released.
    fn write(&mut self, xr: &LibOpenXr, dev: &Device, metres: f32) -> Result<(), String> {
        let d = Self::depth_of(metres);
        let mut index = 0;
        {
            let _queue = lock(&dev.queue_lock);
            unsafe { (xr.xrAcquireSwapchainImage)(self.handle, &XrSwapchainImageAcquireInfo::default(), &mut index) }.to_result("xrAcquireSwapchainImage (depth)")?;
        }
        let wait_info = XrSwapchainImageWaitInfo { timeout: XrDuration(1000000000), ..Default::default() };
        while unsafe { (xr.xrWaitSwapchainImage)(self.handle, &wait_info) } == XrResult::TIMEOUT_EXPIRED {}
        let i = index as usize;
        let recorded = if self.cleared[i] != d {
            let (_, cb, fence) = self.slots[i];
            let image = self.images[i];
            unsafe {
                (|| -> Result<(), vk::Result> {
                    dev.raw.wait_for_fences(&[fence], true, u64::MAX)?;
                    dev.raw.reset_fences(&[fence])?;
                    dev.raw.reset_command_buffer(cb, vk::CommandBufferResetFlags::empty())?;
                    dev.raw.begin_command_buffer(cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))?;
                    let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::DEPTH).level_count(1).layer_count(2);
                    let barrier = |old, new, from, to| vk::ImageMemoryBarrier::default().image(image).old_layout(old).new_layout(new)
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED).src_access_mask(from).dst_access_mask(to).subresource_range(range);
                    dev.raw.cmd_pipeline_barrier(cb, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[],
                        &[barrier(vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::empty(), vk::AccessFlags::TRANSFER_WRITE)]);
                    dev.raw.cmd_clear_depth_stencil_image(cb, image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &vk::ClearDepthStencilValue { depth: d, stencil: 0 }, &[range]);
                    dev.raw.cmd_pipeline_barrier(cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::BOTTOM_OF_PIPE, vk::DependencyFlags::empty(), &[], &[],
                        &[barrier(vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::empty())]);
                    dev.raw.end_command_buffer(cb)?;
                    let cbs = [cb];
                    let _queue = lock(&dev.queue_lock);
                    dev.raw.queue_submit(dev.queue, &[vk::SubmitInfo::default().command_buffers(&cbs)], fence)
                })()
            }.map_err(|e| format!("XR depth layer clear: {e:?}"))
        } else { Ok(()) };
        if recorded.is_ok() { self.cleared[i] = d; } else { self.cleared[i] = f32::NAN; }
        let released = { let _queue = lock(&dev.queue_lock); unsafe { (xr.xrReleaseSwapchainImage)(self.handle, &XrSwapchainImageReleaseInfo::default()) } };
        recorded?;
        released.to_result("xrReleaseSwapchainImage (depth)")
    }

    /// Eye `eye`'s depth for a colour layer of `size` (the chain is the start size; a smaller
    /// eye scale takes its top left)
    fn info(&self, eye: u32, size: [u32; 2]) -> XrCompositionLayerDepthInfoKHR {
        let size = [size[0].min(self.size[0]), size[1].min(self.size[1])];
        XrCompositionLayerDepthInfoKHR { ty: XrStructureType::COMPOSITION_LAYER_DEPTH_INFO_KHR, next: ptr::null(),
            sub_image: XrSwapchainSubImage { swapchain: self.handle, image_rect: XrRect2Di { offset: XrOffset2Di { x: 0, y: 0 },
                extent: XrExtent2Di { width: size[0] as i32, height: size[1] as i32 } }, image_array_index: eye },
            min_depth: 0.0, max_depth: 1.0, near_z: DEPTH_NEAR_M, far_z: DEPTH_FAR_M }
    }

    fn destroy(&mut self, xr: &LibOpenXr, dev: &Device) {
        for (pool, _, fence) in self.slots.drain(..) {
            unsafe {
                let _ = dev.raw.wait_for_fences(&[fence], true, u64::MAX);
                dev.raw.destroy_fence(fence, None);
                dev.raw.destroy_command_pool(pool, None);
            }
        }
        unsafe { (xr.xrDestroySwapchain)(self.handle) }.log_error("xrDestroySwapchain (depth)");
    }
}

/// An XR swapchain, its Vulkan images, a copy slot per image, its size and layers.
/// `times`: summed ns in `write` for acquire, wait for the image, the slot's fence, record + submit.
/// `densities`: each image's fragment density map view (a foveated chain; registered with the
/// GPU backend for its image: passes into it shade coarser where the runtime's profile says).
struct Chain { handle: XrSwapchain, images: Vec<vk::Image>, slots: Vec<Slot>, size: [u32; 2], layers: u32, staging: Option<Arc<Image>>, times: [u64; 5], densities: Vec<vk::ImageView> }

impl Chain {
    /// `swizzle`: the format is RGBA ordered, the frames BGRA: copies go through a staging image
    /// (the size of the frames copied in: `layers` side by side).
    fn new(xr: &LibOpenXr, dev: &Arc<Device>, session: XrSession, format: vk::Format, size: [u32; 2], layers: u32, flags: XrSwapchainCreateFlags, swizzle: bool) -> Result<Chain, String> {
        Self::new_with(xr, dev, session, format, size, layers, flags, swizzle, None, false)
    }

    /// `new` with the swapchain's usage (`None`: the default, mutable format).
    #[allow(clippy::too_many_arguments)]
    fn new_with(xr: &LibOpenXr, dev: &Arc<Device>, session: XrSession, format: vk::Format, size: [u32; 2], layers: u32, flags: XrSwapchainCreateFlags, swizzle: bool, usage: Option<XrSwapchainUsageFlags>, foveated: bool) -> Result<Chain, String> {
        let usage = usage.unwrap_or(XrSwapchainUsageFlags::SAMPLED | XrSwapchainUsageFlags::COLOR_ATTACHMENT | XrSwapchainUsageFlags::TRANSFER_DST | XrSwapchainUsageFlags::MUTABLE_FORMAT);
        let (handle, images, maps) = vulkan::create_swapchain_foveated(xr, session, format, size[0], size[1], layers, flags, usage, foveated)?;
        // the density maps' views (a 2D array: a layer per eye of the multiview pass)
        let mut densities = Vec::new();
        for (image, (map, _, _)) in images.iter().zip(maps.iter()) {
            let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(layers);
            let info = vk::ImageViewCreateInfo::default().image(*map).format(vk::Format::R8G8_UNORM).view_type(vk::ImageViewType::TYPE_2D_ARRAY).subresource_range(range);
            match unsafe { dev.raw.create_image_view(&info, None) } {
                Ok(view) => { crate::backend::vulkan::resources::set_density_map(*image, Some(view)); densities.push(view); }
                Err(e) => { xr_log!("density map view: {e:?}"); break; }
            }
        }
        let slots = images.iter().map(|_| Slot::new(dev)).collect::<Result<_, _>>()?;
        let staging = if swizzle { Some(Self::staging(dev, size, layers, false)?) } else { None };
        Ok(Chain { handle, images, slots, size, layers, staging, times: [0; 5], densities })
    }

    /// The swizzle staging image for frames of this chain: `layers` side by side, or (a
    /// multiview frame) one array layer each.
    fn staging(dev: &Arc<Device>, size: [u32; 2], layers: u32, layered: bool) -> Result<Arc<Image>, String> {
        Image::new(dev.clone(), crate::session::TextureObj { format: crate::format::RGBA8, w: size[0] * if layered { 1 } else { layers }, h: size[1],
            depth: if layered { layers } else { 1 }, mips: 1, usage: crate::usage::RENDER_TARGET | crate::usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: None })
    }

    /// The staging image (when the chain swizzles) for a frame laid out as `frame`.
    fn staging_for(&mut self, dev: &Arc<Device>, frame: &Image) -> Result<Option<Arc<Image>>, String> {
        // a frame already in the chain's order copies as it is
        if frame.desc.format != crate::format::BGRA8 { return Ok(None); }
        let Some(staging) = &self.staging else { return Ok(None) };
        let layered = frame.desc.depth >= self.layers && self.layers > 1;
        if (staging.desc.depth > 1) != layered {
            // (once, when the frames change layout: the copies in flight still read the old one)
            let _queue = lock(&dev.queue_lock);
            unsafe { let _ = dev.raw.device_wait_idle(); }
            self.staging = Some(Self::staging(dev, self.size, self.layers, layered)?);
        }
        Ok(self.staging.clone())
    }

    /// Acquires the chain's next image, records `body` into it (between the barriers that make it
    /// a transfer destination and give it back in COLOR_ATTACHMENT_OPTIMAL), submits and releases
    /// it. `keep` is read by the commands until their fence.
    fn write(&mut self, xr: &LibOpenXr, dev: &Device, keep: Option<Arc<Image>>, body: impl FnOnce(&Device, vk::CommandBuffer, vk::Image)) -> Result<(), String> {
        let mut index = 0;
        let mut mark = Instant::now();
        {
            let _queue = lock(&dev.queue_lock);
            unsafe { (xr.xrAcquireSwapchainImage)(self.handle, &XrSwapchainImageAcquireInfo::default(), &mut index) }.to_result("xrAcquireSwapchainImage")?;
        }
        self.times[0] += mark.elapsed().as_nanos() as u64;
        mark = Instant::now();
        let wait_info = XrSwapchainImageWaitInfo { timeout: XrDuration(1000000000), ..Default::default() };
        loop {
            if unsafe { (xr.xrWaitSwapchainImage)(self.handle, &wait_info) } != XrResult::TIMEOUT_EXPIRED { break; }
            xr_log!("retry xrWaitSwapchainImage");
        }
        self.times[1] += mark.elapsed().as_nanos() as u64;
        let dst = self.images[index as usize];
        let layers = self.layers;
        let slot = &mut self.slots[index as usize];
        let fence_ns = slot.wait(dev);
        if fence_ns != u64::MAX { self.times[2] += fence_ns; }
        mark = Instant::now();
        let recorded = fence_ns_ok(fence_ns).and_then(|_| slot.record(dev, dst, layers, |dev, cb| body(dev, cb, dst)));
        if recorded.is_ok() { slot.keep = keep; }
        self.times[3] += mark.elapsed().as_nanos() as u64;
        mark = Instant::now();
        let released = {
            let _queue = lock(&dev.queue_lock);
            unsafe { (xr.xrReleaseSwapchainImage)(self.handle, &XrSwapchainImageReleaseInfo::default()) }
        };
        self.times[4] += mark.elapsed().as_nanos() as u64;
        recorded?;
        released.to_result("xrReleaseSwapchainImage")
    }

    /// The summed write times (ms per write over `n` writes), reset.
    fn take_times(&mut self, n: u64) -> [f64; 7] {
        let t = self.times.map(|v| v as f64 / 1e6 / n.max(1) as f64);
        self.times = [0; 5];
        let (mut lock, mut submit) = (0, 0);
        let mut steps = [0u64; 4];
        for s in &mut self.slots { lock += s.lock_ns; submit += s.submit_ns; s.lock_ns = 0; s.submit_ns = 0; for i in 0..4 { steps[i] += s.steps[i]; } s.steps = [0; 4]; }
        let st = steps.map(|v| v as f64 / 1e6 / n.max(1) as f64);
        xr_log!("copy record ms: reset+begin {:.2} barrier {:.2} body {:.2} barrier+end {:.2}", st[0], st[1], st[2], st[3]);
        [t[0], t[1], t[2], t[3], lock as f64 / 1e6 / n.max(1) as f64, submit as f64 / 1e6 / n.max(1) as f64, t[4]]
    }

    fn destroy(&mut self, xr: &LibOpenXr, dev: &Device) {
        for image in &self.images { crate::backend::vulkan::resources::set_density_map(*image, None); }
        for view in self.densities.drain(..) { unsafe { dev.raw.destroy_image_view(view, None) }; }
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
            Ok(Slot { pool, cb, fence, keep: None, lock_ns: 0, submit_ns: 0, steps: [0; 4] })
        }
    }

    /// Waits for the slot's last copy (its fence), then drops what it kept; the wait in ns
    /// (`u64::MAX`: the wait failed).
    fn wait(&mut self, dev: &Device) -> u64 {
        let at = Instant::now();
        if unsafe { dev.raw.wait_for_fences(&[self.fence], true, u64::MAX) }.is_err() { return u64::MAX; }
        self.keep = None;
        at.elapsed().as_nanos() as u64
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
        let mut at = Instant::now();
        let mut step = |i: usize, steps: &mut [u64; 4]| { steps[i] += at.elapsed().as_nanos() as u64; at = Instant::now(); };
        unsafe {
            dev.raw.reset_fences(&[self.fence]).map_err(|e| format!("XR copy fence reset: {e:?}"))?;
            dev.raw.reset_command_buffer(self.cb, vk::CommandBufferResetFlags::empty()).map_err(|e| format!("XR copy reset: {e:?}"))?;
            dev.raw.begin_command_buffer(self.cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(|e| format!("XR copy begin: {e:?}"))?;
            step(0, &mut self.steps);
            dev.raw.cmd_pipeline_barrier(self.cb, vk::PipelineStageFlags::ALL_COMMANDS, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default().src_access_mask(vk::AccessFlags::MEMORY_WRITE).dst_access_mask(vk::AccessFlags::TRANSFER_READ)], &[],
                &[barrier(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::COLOR_ATTACHMENT_WRITE, vk::AccessFlags::TRANSFER_WRITE)]);
            step(1, &mut self.steps);
            body(dev, self.cb);
            step(2, &mut self.steps);
            dev.raw.cmd_pipeline_barrier(self.cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, vk::DependencyFlags::empty(), &[], &[],
                &[barrier(vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE)]);
            dev.raw.end_command_buffer(self.cb).map_err(|e| format!("XR copy end: {e:?}"))?;
            step(3, &mut self.steps);
            let cbs = [self.cb];
            let at = Instant::now();
            let _queue = lock(&dev.queue_lock);
            self.lock_ns += at.elapsed().as_nanos() as u64;
            let at = Instant::now();
            dev.raw.queue_submit(dev.queue, &[vk::SubmitInfo::default().command_buffers(&cbs)], self.fence).map_err(|e| format!("XR copy submit: {e:?}"))?;
            self.submit_ns += at.elapsed().as_nanos() as u64;
        }
        Ok(())
    }
}

fn fence_ns_ok(ns: u64) -> Result<(), String> {
    if ns == u64::MAX { Err("XR copy fence wait failed".into()) } else { Ok(()) }
}

/// Copies a frame image (BGRA8 UNORM in TRANSFER_SRC_OPTIMAL, as the core hands it over) into
/// `dst` bit for bit: `layers` regions of `size` side by side in the frame go to layers
/// 0..layers (the eyes of a stereo frame; the whole frame for a one-layer chain). The frames
/// hold sRGB-encoded colour and the swapchain is sRGB. A swapchain in RGBA order goes through
/// `staging` (RGBA8 UNORM): a blit there only swaps the channels, then the copy.
fn copy_frame(dev: &Device, cb: vk::CommandBuffer, src: &Image, dst: vk::Image, size: [u32; 2], layers: u32, staging: Option<&Image>) {
    let sub = |layer| vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).base_array_layer(layer).layer_count(1);
    // a multiview frame holds the views as layers, else side by side in layer 0
    let layered = src.desc.depth >= layers && layers > 1;
    let copies: Vec<vk::ImageCopy> = (0..layers).map(|l| if layered {
        vk::ImageCopy::default().src_subresource(sub(l)).dst_subresource(sub(l)).extent(vk::Extent3D { width: size[0], height: size[1], depth: 1 })
    } else {
        vk::ImageCopy::default().src_subresource(sub(0)).dst_subresource(sub(l))
            .src_offset(vk::Offset3D { x: (l * size[0]) as i32, y: 0, z: 0 }).extent(vk::Extent3D { width: size[0], height: size[1], depth: 1 })
    }).collect();
    unsafe {
        let Some(staging) = staging else {
            dev.raw.cmd_copy_image(cb, src.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, dst, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &copies);
            return;
        };
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(staging.desc.depth);
        let barrier = |old, new, src_access, dst_access| vk::ImageMemoryBarrier::default().image(staging.image).old_layout(old).new_layout(new)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .src_access_mask(src_access).dst_access_mask(dst_access).subresource_range(range);
        dev.raw.cmd_pipeline_barrier(cb, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[],
            &[barrier(vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::TRANSFER_READ, vk::AccessFlags::TRANSFER_WRITE)]);
        // (the staging image has the frame's layout: `Chain::staging_for`)
        let corner = vk::Offset3D { x: (size[0] * if layered { 1 } else { layers }) as i32, y: size[1] as i32, z: 1 };
        let all = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(if layered { layers } else { 1 });
        let region = vk::ImageBlit::default().src_subresource(all).dst_subresource(all)
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
    /// the environment depth readback (None without a depth provider)
    depth: Option<depth::DepthSource>,
    inputs: Arc<Mutex<SharedInputs>>,
    /// the tracking thread (`track`) and its stop flag
    tracker: Option<(std::thread::JoinHandle<()>, Arc<AtomicBool>)>,
    active: bool,
    focused: bool,
    active_display_refresh_rate_hz: Option<f32>,
    panel: Chain,
    cursor: Chain,
    /// the projection layer: a layer per eye (the app's XR window renders into its images)
    stereo: Option<Chain>,
    pointer: Pointer,
    controllers: [XrController; 2],
    hands: [XrHand; 2],
    /// a panel frame was released into its chain (its layer may be shown), and its size
    panel_shown: Option<[u32; 2]>,
    /// the panel shows (always without an XR window; with one, `panel_over_stereo`)
    panel_visible: bool,
    /// the panel shows over the stereo view (toggled by the left thumbstick click; off at start)
    panel_over_stereo: bool,
    /// a following panel's place (`hal_xr_panel` with follow)
    panel_follow: Option<PanelFollow>,
    cursor_ready: bool,
    /// the eyes the stereo frame in flight was asked for, and the eyes of the frame the
    /// projection layer shows
    stereo_asked: Option<[Eye; 2]>,
    stereo_shown: Option<[Eye; 2]>,
    /// the depth of what the shown stereo frame shows (metres; 0: no depth layer)
    stereo_focus: f32,
    /// the projection layer's depth swapchain
    depth_layer: Option<DepthChain>,
    stereo_asked_at: Instant,
    /// counters for the periodic timing line: display frames, stereo frames shown, frames
    /// whose stereo frame came too late, and the summed wait for stereo frames (ns)
    stats: [u64; 4],
    /// per stereo frame taken (µs): asked to scheduled (the app's CPU), scheduled to arrived
    /// (recorded and on the queue), asked to arrived, and the time it had (asked to the take's
    /// deadline)
    frame_us: Vec<[u32; 4]>,
    stereo_asked_ns: u64,
    /// the refresh rate last requested (mHz; 0 none)
    requested_refresh_mhz: u32,
    /// stereo frames taken this window with eyes of their own (`HalCmd::XrFrameEyes`), and
    /// without (shown with their XrFrame's)
    eyes_own: u64,
    eyes_asked: u64,
    /// stereo frames taken per path this window: rendered into the chain, copied in, dropped
    paths: [u64; 3],
    /// summed time (ns) per part of the frame loop: xrWaitFrame, xrBeginFrame, input, panel,
    /// stereo (ask + wait), stereo copy, xrEndFrame
    phases: [u64; 7],
    stats_at: Instant,
    /// the second projection chain the app renders into (frames alternate between the two)
    stereo_b: Option<Chain>,
    /// the projection chain of the stereo frame shown
    stereo_shown_chain: Option<XrSwapchain>,
    /// what the projection chains are made from (their format, swizzled copies) and whether
    /// the runtime offers an sRGB format for linear frames (`hal_xr_linear_frames`)
    stereo_base_format: vk::Format,
    stereo_blit: bool,
    linear_ok: bool,
    /// the foveation level the projection chains have (`apply_foveation`; MAX: not yet)
    foveation_applied: u32,
    /// the dynamic eye scale's controller
    eye_scale: EyeScale,
    /// projection chains of other eye sizes, kept (`ChainSet`), and the set being built for a
    /// size the scale is going to (its percent)
    chain_spare: Vec<ChainSet>,
    chain_build: Option<(u32, std::thread::JoinHandle<Result<ChainSet, String>>)>,
    /// simultaneous hands and controllers tracking is resumed (the inputs resume it at start)
    hands_and_controllers: bool,
    /// the environment depth provider runs (started with the session)
    environment_depth: bool,
}

/// The dynamic eye scale (`hal_xr_eye_scale`): the eye buffers step between 100, 90, 80 and 70%
/// of the start size on the runtime's app GPU frame time (XR_META_performance_metrics' counter,
/// the time VrApi logs as App), so a heavier view never drops display frames. It decides from
/// what it MEASURED at each size (the median of its half-second means there, from 1 s after
/// the change), not from a model: the app's time barely follows the pixels (70%: 7.0-7.6 ms,
/// 80%: 7.2-10.3, 90%: 6.6-8.4 on one race view), and a model that scaled it with them stepped
/// up, missed, stepped down again every few seconds (each step a frame or two of the
/// compositor's). Down: a sustained overload (3 s of means over 85% of the period) and the
/// smaller size not measured to cost about as much (a step that did not lower the time is not
/// taken again). Up: at most every 30 s, when the bigger size measured (else predicted from
/// this one by its pixels) under 70% of the period, or measured no dearer than this one. A
/// steady view picks its size and stays.
struct EyeScale { counter: Option<XrPath>, sum_ms: f64, n: u32, window_at: Instant, over: u32, changed_at: Instant, log_sum_ms: f64, log_n: u32,
    /// per step (`STEPS` index): its recent settled half-second means (ms)
    cost: [std::collections::VecDeque<f64>; 4] }

impl EyeScale {
    const STEPS: [u32; 4] = [100, 90, 80, 70];
    fn new(xr: &LibOpenXr, instance: XrInstance, session: XrSession) -> EyeScale {
        let counter = (|| {
            let set = xr.xrSetPerformanceMetricsStateMETA?;
            xr.xrQueryPerformanceMetricsCounterMETA?;
            let state = XrPerformanceMetricsStateMETA { ty: XrStructureType::PERFORMANCE_METRICS_STATE_META, next: ptr::null(), enabled: XrBool32::from_bool(true) };
            unsafe { set(session, &state) }.to_result("xrSetPerformanceMetricsStateMETA").map_err(|e| xr_log!("{e}")).ok()?;
            XrPath::new(xr, instance, "/perfmetrics_meta/app/gpu_frametime").map_err(|e| xr_log!("{e}")).ok()
        })();
        xr_log!("dynamic eye scale: {}", if counter.is_some() { "the runtime's app GPU frame time" } else { "no performance metrics (fixed size)" });
        EyeScale { counter, sum_ms: 0.0, n: 0, window_at: Instant::now(), over: 0, changed_at: Instant::now(), log_sum_ms: 0.0, log_n: 0, cost: Default::default() }
    }

    /// The measured cost of step `i` (the median of its settled means; None: not measured).
    fn cost_of(&self, i: usize) -> Option<f64> {
        let c = &self.cost[i];
        if c.len() < 4 { return None; }
        let mut v: Vec<f64> = c.iter().copied().collect();
        v.sort_by(|a, b| a.total_cmp(b));
        Some(v[v.len() / 2])
    }

    /// One display frame: the counter sampled; the percent to change to, when it is time.
    fn sample(&mut self, xr: &LibOpenXr, session: XrSession, period_ms: f64) -> Option<u32> {
        let (counter, query) = (self.counter?, xr.xrQueryPerformanceMetricsCounterMETA?);
        let mut c = XrPerformanceMetricsCounterMETA { ty: XrStructureType::PERFORMANCE_METRICS_COUNTER_META, next: ptr::null(), counter_flags: 0, counter_unit: 0, uint_value: 0, float_value: 0.0 };
        if unsafe { query(session, counter, &mut c) }.to_result("xrQueryPerformanceMetricsCounterMETA").is_ok() {
            // (milliseconds as a float, else an integer in µs)
            let ms = if c.counter_flags & 4 != 0 { c.float_value as f64 } else if c.counter_flags & 2 != 0 { c.uint_value as f64 / 1000.0 } else { 0.0 };
            if ms > 0.0 && ms < 1000.0 { self.sum_ms += ms; self.n += 1; self.log_sum_ms += ms; self.log_n += 1; }
        }
        if self.window_at.elapsed() < Duration::from_millis(500) { return None; }
        self.window_at = Instant::now();
        let mean = if self.n > 0 { self.sum_ms / self.n as f64 } else { return None };
        (self.sum_ms, self.n) = (0.0, 0);
        EYE_GPU_US.store((mean * 1000.0) as u32, Ordering::Release);
        let range = EYE_PERCENT_RANGE.load(Ordering::Acquire);
        let (min, max) = ((range >> 32) as u32, range as u32);
        let now = EYE_PERCENT.load(Ordering::Acquire);
        if now > max || now < min { return Some(now.clamp(min, max)); }
        let since = self.changed_at.elapsed();
        let Some(here) = Self::STEPS.iter().position(|&p| p == now) else { return None };
        // (settled at this size: its cost)
        if since >= Duration::from_secs(1) {
            let c = &mut self.cost[here];
            c.push_back(mean);
            if c.len() > 20 { c.pop_front(); }
        }
        let down = (here + 1 < Self::STEPS.len() && Self::STEPS[here + 1] >= min).then_some(here + 1);
        let up = (here > 0 && Self::STEPS[here - 1] <= max).then(|| here - 1);
        self.over = if mean > 0.85 * period_ms { self.over + 1 } else { 0 };
        let cost_here = self.cost_of(here).unwrap_or(mean);
        let to = if self.over >= 6 {
            // a smaller size that measured about as dear does not help
            down.filter(|&d| self.cost_of(d).is_none_or(|c| c < cost_here - 0.5))
        } else if since >= Duration::from_secs(30) && self.over == 0 {
            up.filter(|&u| match self.cost_of(u) {
                Some(c) => c < 0.70 * period_ms || c <= cost_here + 0.3,
                None => cost_here * (Self::STEPS[u] as f64 / now as f64).powi(2) < 0.70 * period_ms,
            })
        } else {
            None
        };
        let to = to.map(|i| Self::STEPS[i]);
        if to.is_some() {
            self.over = 0;
            self.changed_at = Instant::now();
        }
        to
    }

    /// The mean app GPU frame time since the last call (ms; the 5 s line).
    fn take_mean(&mut self) -> f64 {
        let m = if self.log_n > 0 { self.log_sum_ms / self.log_n as f64 } else { 0.0 };
        (self.log_sum_ms, self.log_n) = (0.0, 0);
        m
    }
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
            let r = unsafe { (xr.xrSetEnvironmentDepthHandRemovalMETA)(depth_provider, &edhrsi) };
            xr_log!("environment depth provider {:#x} created; hand removal {}: {r:?}", depth_provider.0, options.remove_hands_from_depth);
            r.to_result("xrSetEnvironmentDepthHandRemovalMETA")?;

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
        frames.rgba.store(blit, Ordering::Release);
        let (passthrough, passthrough_layer, depth_provider, depth_swap_chain) = Self::create_passthrough_and_depth(xr, handle, options)?;
        let inputs = Arc::new(Mutex::new(SharedInputs(CxOpenXrInputs::new_inputs(xr, handle, rt.instance)?)));
        xr_log!("session {handle:?}: local space {local_space_type:?}, swapchain {format:?}{}; passthrough running",
            if blit { " (swizzled through staging)" } else { "" });
        let panel = Chain::new(xr, &dev, handle, format, PANEL_PX, 1, XrSwapchainCreateFlags(0), blit)?;
        // the projection chain: the app's XR window renders straight into its images
        let eye = EYE_PX.get().copied().unwrap_or([1, 1]);
        let linear_ok = formats.contains(&i64::from(vk::Format::R8G8B8A8_SRGB.as_raw()));
        let linear = linear_ok && LINEAR_WANTED.load(Ordering::Acquire);
        let (stereo, stereo_b, targets) = stereo_chains(xr, &dev, handle, format, eye, blit, linear)?;
        LINEAR_ACTIVE.store(linear, Ordering::Release);
        *lock(&frames.stereo_targets) = Some(targets);
        let depth_layer = DepthChain::new(xr, &dev, handle, eye).map_err(|e| xr_log!("no projection depth layer: {e}")).ok();
        let cursor = Chain::new(xr, &dev, handle, format, [CURSOR_PX; 2], 1, XrSwapchainCreateFlags::STATIC_IMAGE, false)?;
        let depth = depth::DepthSource::new(xr, &dev, depth_provider, depth_swap_chain).unwrap_or_else(|e| {
            xr_log!("environment depth readback unavailable: {e}");
            None
        });
        let mut session = Self { depth, rt: rt.clone(), dev, frames, handle, head_space, local_space, panel_space, passthrough, passthrough_layer, depth_provider, depth_swap_chain,
            inputs: inputs.clone(), tracker: None, active: false, focused: false, active_display_refresh_rate_hz: None, panel, cursor, stereo: Some(stereo),
            pointer: Pointer { hand: 1, ..Default::default() }, controllers: Default::default(), hands: Default::default(),
            panel_shown: None, panel_visible: true, panel_over_stereo: false, panel_follow: None, cursor_ready: false, stereo_asked: None, stereo_shown: None, stereo_focus: 0.0, depth_layer, stereo_asked_at: Instant::now(),
            stats: [0; 4], frame_us: Vec::new(), stereo_asked_ns: 0, requested_refresh_mhz: 0, eyes_own: 0, eyes_asked: 0, paths: [0; 3], phases: [0; 7], stats_at: Instant::now(), stereo_b: Some(stereo_b), stereo_shown_chain: None,
            stereo_base_format: format, stereo_blit: blit, linear_ok, foveation_applied: u32::MAX, eye_scale: EyeScale::new(xr, rt.instance, handle), chain_spare: Vec::new(), chain_build: None, hands_and_controllers: true, environment_depth: true };
        let _ = session.update_active_display_refresh_rate();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (rt, inputs, stop) = (rt.clone(), inputs, stop.clone());
            std::thread::Builder::new().name("hal-xr-track".into())
                .spawn(move || track(rt, inputs, handle, local_space, head_space, stop)).map_err(|e| format!("XR tracking thread: {e}"))?
        };
        session.tracker = Some((thread, stop));
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

    /// Requests the refresh rate the XR window asked for (`HalCmd::XrRefreshRate`) once it
    /// changes: the supported rate nearest to it.
    fn apply_refresh_rate(&mut self) {
        let want = REFRESH_MHZ.load(Ordering::Acquire);
        if want == self.requested_refresh_mhz { return; }
        self.requested_refresh_mhz = want;
        let (Some(enumerate), Some(request)) = (self.rt.xr.xrEnumerateDisplayRefreshRatesFB, self.rt.xr.xrRequestDisplayRefreshRateFB) else {
            xr_log!("no display refresh rate control on this runtime");
            return;
        };
        let mut n = 0u32;
        unsafe { (enumerate)(self.handle, 0, &mut n, ptr::null_mut()) };
        let mut rates = vec![0.0f32; n as usize];
        unsafe { (enumerate)(self.handle, n, &mut n, rates.as_mut_ptr()) };
        rates.truncate(n as usize);
        // 0: the runtime picks (a request of 0 asks for its default)
        let hz = if want == 0 { 0.0 } else {
            let w = want as f32 / 1000.0;
            rates.iter().copied().min_by(|a, b| (a - w).abs().total_cmp(&(b - w).abs())).unwrap_or(0.0)
        };
        match unsafe { (request)(self.handle, hz) }.to_result("xrRequestDisplayRefreshRateFB") {
            Ok(()) => xr_log!("display refresh rate {hz:.1} Hz requested (asked {:.1}; supported {rates:?})", want as f32 / 1000.0),
            Err(e) => xr_log!("{e}"),
        }
    }

    fn frame(&mut self) -> Result<(), String> {
        self.apply_refresh_rate();
        let rt = self.rt.clone();
        let xr = &rt.xr;
        let mut fi = XrFrameWaitInfo::default();
        let mut frame_state = XrFrameState::default();
        let mut mark = Instant::now();
        let mut phase = |i: usize, phases: &mut [u64; 7]| { phases[i] += mark.elapsed().as_nanos() as u64; mark = Instant::now(); };
        unsafe { (xr.xrWaitFrame)(self.handle, &mut fi, &mut frame_state) }.to_result("xrWaitFrame")?;
        phase(0, &mut self.phases);
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
            // FRAME_DISCARDED is a success: the frame before was not ended (it is dropped)
            let begun = unsafe { (xr.xrBeginFrame)(self.handle, &XrFrameBeginInfo::default()) };
            if begun == XrResult::FRAME_DISCARDED { crate::profile::count("xr frame discarded"); } else { begun.to_result("xrBeginFrame")?; }
        }
        phase(1, &mut self.phases);
        let previous = self.controllers;
        PERIOD_NS.store(period, Ordering::Release);
        *lock(&DISPLAY_FRAME) = time.as_nanos() as u64;
        DISPLAY_FRAME_NEW.notify_all();
        if uplink_wanted() {
            // the tracking thread polls the input (its newest)
            (self.controllers, self.hands) = lock(&TRACKED_INPUT).unwrap_or_default();
        } else {
            match lock(&self.inputs).0.poll(xr, self.handle, self.local_space, time) {
                Some((controllers, hands)) => { self.controllers = controllers; self.hands = hands; }
                // not focused: nothing is held
                None => { self.controllers = Default::default(); self.hands = Default::default(); }
            }
        }
        let xr_window = lock(&shared.registry).xr_window;
        if xr_window.is_some() {
            // over the stereo view the panel is hidden (it would cover the scene: layers are not
            // depth tested) until a click of a held left controller's thumbstick shows it (and
            // the next hides it); hands never toggle it, the face and menu buttons stay the app's
            let click = |c: &XrController| c.buttons & xr_controller::ACTIVE != 0 && c.buttons & xr_controller::CLICK_THUMBSTICK != 0;
            if click(&self.controllers[0]) && !click(&previous[0]) { self.panel_over_stereo = !self.panel_over_stereo; }
            self.panel_visible = self.panel_over_stereo;
        } else {
            self.panel_visible = true;
        }
        let (show, distance, width, follow) = *lock(&PANEL_LAYOUT);
        match show {
            1 => self.panel_visible = true,
            2 => self.panel_visible = false,
            _ => {}
        }
        let panel_m = [width, width * PANEL_PX[1] as f32 / PANEL_PX[0] as f32];
        let panel_pose = if follow {
            self.follow_panel(time, period as u64, distance)
        } else {
            self.panel_follow = None;
            // the panel's plane in the local space: LOCAL_FLOOR is LOCAL lowered to the floor
            let mut loc = XrSpaceLocation::locate(xr, self.local_space, time, self.panel_space).pose;
            loc.position.z -= distance;
            loc.orientation = Default::default();
            loc.orientation.w = 1.;
            loc
        };
        // (a following panel is display only: the pointer is for the fixed one)
        let hit = if self.panel_visible && !follow { self.point(panel_pose) } else { self.leave_panel(); None };
        phase(2, &mut self.phases);
        // from here on every frame ends (a frame begun and not ended discards the next one)
        // a hidden panel's frames are let go uncopied (no GPU work for a layer not shown)
        let panel_done = if render && self.panel_visible { self.update_panel().unwrap_or_else(|e| { xr_log!("panel: {e}"); None }) }
            else { lock(&self.frames.panel).take().map(|(_, done, _)| done) };
        phase(3, &mut self.phases);
        self.switch_stereo_chains();
        self.apply_foveation();
        self.step_eye_scale(xr_window, period);
        self.apply_hands_and_controllers();
        self.apply_environment_depth();
        // (paused: nothing asked and nothing shown, the room alone; a frame still in flight
        // waits in its slot and is taken, with its chain image, when the view resumes)
        let paused = STEREO_PAUSED.load(Ordering::Acquire);
        if paused && self.stereo_shown.is_some() {
            self.stereo_shown = None;
            self.stereo_asked = None;
        }
        let stereo_done = match xr_window { Some((sid, id)) if render && !paused => self.update_stereo(sid, id, time, period).unwrap_or_else(|e| { xr_log!("stereo: {e}"); None }), _ => None };
        phase(4, &mut self.phases);
        // environment depth: a finished readback to the XR window, the next one started (never waits)
        if let (Some(depth), Some((sid, id))) = (self.depth.as_mut(), xr_window) {
            if let Some(payload) = depth.poll(&self.dev) {
                let t = time.as_nanos() as u64;
                shared.emit_to(sid, id, clock_ns(), |q, id| q.xr_depth(id, t, &payload));
            }
            if let Some(header) = depth.acquire(xr, &self.dev, self.depth_provider, self.local_space, time) {
                let mut payload = Vec::with_capacity(<crate::XrDepth as crate::buffers::Wire>::SIZE);
                crate::buffers::Wire::write(&header, &mut payload);
                let t = time.as_nanos() as u64;
                shared.emit_to(sid, id, clock_ns(), |q, id| q.xr_depth(id, t, &payload));
            }
        }
        if !self.cursor_ready && render { if let Err(e) = self.fill_cursor() { xr_log!("cursor: {e}"); } }

        let quad = |chain: &Chain, size: [u32; 2], pose, metres: [f32; 2]| {
            let mut sub_image = chain.sub_image(0);
            sub_image.image_rect.extent = XrExtent2Di { width: size[0] as i32, height: size[1] as i32 };
            XrCompositionLayerQuad { ty: XrStructureType::COMPOSITION_LAYER_QUAD, next: ptr::null(), layer_flags: XrCompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA,
                space: self.local_space, eye_visibility: XrEyeVisibility::BOTH, sub_image, pose, size: XrExtent2Df { width: metres[0], height: metres[1] } }
        };
        let panel = self.panel_shown.filter(|_| self.panel_visible).map(|size| quad(&self.panel, size, panel_pose, panel_m));
        let cursor = hit.filter(|_| self.cursor_ready && panel.is_some()).map(|p| {
            let mut pose = panel_pose;
            pose.position.x = p[0]; pose.position.y = p[1]; pose.position.z += 0.003;
            quad(&self.cursor, [CURSOR_PX; 2], pose, [CURSOR_M; 2])
        });
        // the old end_frame: passthrough under a projection that blends with its alpha
        let comp_passthrough = XrCompositionLayerPassthroughFB { layer_handle: self.passthrough_layer, flags: XrCompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA, ..Default::default() };
        // the depth of what the frame shows, for the compositor's positional correction
        let focus = self.stereo_focus;
        if focus > 0.0 && render {
            if let Some(depth) = self.depth_layer.as_mut() {
                if let Err(e) = depth.write(xr, &self.dev, focus) { xr_log!("{e}"); }
            }
        }
        let colour_size = self.stereo.as_ref().map_or([1, 1], |c| c.size);
        let depth_infos = self.depth_layer.as_ref().filter(|_| focus > 0.0).map(|d| [d.info(0, colour_size), d.info(1, colour_size)]);
        let proj_views = match (&self.stereo, self.stereo_shown) {
            (Some(chain), Some(eyes)) => Some([0, 1].map(|eye| XrCompositionLayerProjectionView { pose: eyes[eye].pose, fov: eyes[eye].fov,
                sub_image: XrSwapchainSubImage { swapchain: self.stereo_shown_chain.unwrap_or(chain.handle), ..chain.sub_image(eye as u32) },
                next: depth_infos.as_ref().map_or(ptr::null(), |d| &d[eye] as *const _ as *const c_void), ..Default::default() })),
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
        phase(6, &mut self.phases);

        let shown_at = time.as_nanos() as u64;
        if let Some(done) = panel_done { done(shown_at); }
        if let Some(done) = stereo_done { done(shown_at); }
        self.stats[0] += 1;
        if self.stats_at.elapsed() >= Duration::from_secs(5) {
            let [frames, shown, late, wait] = self.stats;
            let secs = self.stats_at.elapsed().as_secs_f64();
            let hands = self.hands.map(|h| if h.flags & xr_hand::IN_VIEW != 0 { "tracked" } else { "-" });
            let ms = self.phases.map(|v| v as f64 / 1e6 / frames.max(1) as f64);
            let depth = self.depth.as_mut().map(|d| { let n = (d.acquired, d.delivered); d.acquired = 0; d.delivered = 0; n });
            // the projection layer's views against the views located for this display time
            if let (Some(views), Some((_, now))) = (proj_views.as_ref(), self.locate(time)) {
                for eye in 0..2 {
                    let (v, n) = (&views[eye], &now[eye]);
                    let q = |p: &XrPosef| [p.orientation.x, p.orientation.y, p.orientation.z, p.orientation.w];
                    let t = |p: &XrPosef| [p.position.x, p.position.y, p.position.z];
                    xr_log!("layer eye {eye}: pose {:?} {:?} fov {:?} layer {} {}x{} | located {:?} {:?} fov {:?}", q(&v.pose), t(&v.pose),
                        [v.fov.angle_left, v.fov.angle_right, v.fov.angle_up, v.fov.angle_down], v.sub_image.image_array_index, v.sub_image.image_rect.extent.width, v.sub_image.image_rect.extent.height,
                        q(&n.pose), t(&n.pose), [n.fov.angle_left, n.fov.angle_right, n.fov.angle_up, n.fov.angle_down]);
                }
            }
            if let Some(chain) = self.stereo.as_mut() {
                let t = chain.take_times(shown);
                xr_log!("stereo copy per frame ms: acquire {:.2} wait-image {:.2} slot-fence {:.2} record+submit {:.2} (queue lock {:.2} vkQueueSubmit {:.2}) release {:.2}", t[0], t[1], t[2], t[3], t[4], t[5], t[6]);
            }
            xr_log!("{frames} display frames in {secs:.1} s ({:.1} Hz): {shown} stereo frames shown, {late} late, {:.2} ms average wait; per frame ms: wait-frame {:.2} begin {:.2} input {:.2} panel {:.2} stereo {:.2} (copy {:.2}) end {:.2}; panel {}; hands {hands:?}; focused {}; depth {}",
                frames as f64 / secs, wait as f64 / 1e6 / shown.max(1) as f64, ms[0], ms[1], ms[2], ms[3], ms[4], ms[5], ms[6], if !self.panel_visible { "hidden" } else if self.panel_shown.is_some() { "up" } else { "no frame yet" }, self.focused,
                depth.map(|(a, d)| format!("{a} read, {d} sent")).unwrap_or_else(|| "none".into()));
            xr_log!("stereo frames: {} rendered into the chain, {} copied in, {} dropped; shown with their own eyes {}, with the asked eyes {}; eye scale {}% ({}x{}), app GPU {:.2} ms", self.paths[0], self.paths[1], self.paths[2], self.eyes_own, self.eyes_asked,
                EYE_PERCENT.load(Ordering::Acquire), self.stereo.as_ref().map_or(0, |c| c.size[0]), self.stereo.as_ref().map_or(0, |c| c.size[1]), self.eye_scale.take_mean());
            (self.eyes_own, self.eyes_asked) = (0, 0);
            {
                let mut cols: [Vec<u32>; 4] = Default::default();
                for f in self.frame_us.drain(..) { for i in 0..4 { cols[i].push(f[i]); } }
                let q = |v: &mut Vec<u32>| { v.sort_unstable(); let at = |f: f64| v.get(((v.len() as f64 - 1.0) * f) as usize).map_or(0.0, |x| *x as f64 / 1000.0); format!("{:.1}/{:.1}/{:.1}", at(0.5), at(0.9), at(1.0)) };
                let [a, b, c, d] = &mut cols;
                let (wait, waits) = (TARGET_WAIT_NS.swap(0, Ordering::Relaxed), TARGET_WAITS.swap(0, Ordering::Relaxed));
                xr_log!("stereo frame ms p50/p90/max: app cpu {} to queue {} asked->arrived {} (had {}); chain image acquire+wait {:.2} ms avg over {waits}", q(a), q(b), q(c), q(d), wait as f64 / 1e6 / waits.max(1) as f64);
            }
            self.paths = [0; 3];
            self.stats = [0; 4];
            self.phases = [0; 7];
            self.stats_at = Instant::now();
        }
        r.to_result("xrEndFrame")
    }

    /// The projection chains made anew when the app's frames changed between linear and
    /// sRGB-encoded colour (`hal_xr_linear_frames`), at a frame boundary: no frame asked, no
    /// image held. The frame's layer is left out once (its chain is gone).
    fn switch_stereo_chains(&mut self) {
        let wanted = self.linear_ok && LINEAR_WANTED.load(Ordering::Acquire);
        if wanted == LINEAR_ACTIVE.load(Ordering::Acquire) || self.stereo_asked.is_some() { return; }
        let mut targets = lock(&self.frames.stereo_targets);
        if targets.as_ref().is_some_and(|t| !t.acquired.is_empty()) { return; }
        let eye = self.stereo.as_ref().map_or_else(|| eye_at(EYE_PERCENT.load(Ordering::Acquire)), |c| c.size);
        match stereo_chains(&self.rt.xr, &self.dev, self.handle, self.stereo_base_format, eye, self.stereo_blit, wanted) {
            Ok((a, b, t)) => {
                *targets = Some(t);
                drop(targets);
                if let Some(mut old) = self.stereo.replace(a) { old.destroy(&self.rt.xr, &self.dev); }
                if let Some(mut old) = self.stereo_b.replace(b) { old.destroy(&self.rt.xr, &self.dev); }
                self.stereo_shown = None;
                self.stereo_shown_chain = None;
                self.foveation_applied = u32::MAX;
                LINEAR_ACTIVE.store(wanted, Ordering::Release);
            }
            Err(e) => { xr_log!("projection chains for linear frames: {e}"); self.linear_ok = false; }
        }
    }

    /// The dynamic eye scale (`EyeScale`): a new size goes to the XR window as its geometry
    /// (the app's next frames render at it; the chains follow the first such frame,
    /// `update_stereo`) once projection chains of that size exist: they are built on a thread of
    /// their own first (building them in the XR thread took ~10 ms of a frame). Chains of a size
    /// left are kept; ones unused for a second beyond the newest spare are let go on a thread
    /// (no frame, and so no fallback the compositor shows, refers to them any more).
    fn step_eye_scale(&mut self, xr_window: Option<(u32, Id)>, period: i64) {
        // a set built: kept, and the size it was built for goes to the app
        if self.chain_build.as_ref().is_some_and(|(_, h)| h.is_finished()) {
            let (to, build) = self.chain_build.take().unwrap();
            match build.join() {
                Ok(Ok(set)) => { self.chain_spare.push(set); self.apply_eye_percent(to, xr_window, period); }
                Ok(Err(e)) => xr_log!("projection chains for {to}%: {e}"),
                Err(_) => xr_log!("projection chains for {to}%: the builder panicked"),
            }
        }
        self.retire_spare_chains();
        let Some(to) = self.eye_scale.sample(&self.rt.xr, self.handle, period as f64 / 1e6) else { return };
        if to == EYE_PERCENT.load(Ordering::Acquire) || self.chain_build.is_some() { return; }
        let eye = eye_at(to);
        if self.stereo.as_ref().is_some_and(|c| c.size == eye) || self.chain_spare.iter().any(|c| c.size == eye) {
            self.apply_eye_percent(to, xr_window, period);
            return;
        }
        let (rt, dev, session, format, blit, linear) = (self.rt.clone(), self.dev.clone(), self.handle, self.stereo_base_format, self.stereo_blit, LINEAR_ACTIVE.load(Ordering::Acquire));
        let started = Instant::now();
        let session = SendSession(session);
        match std::thread::Builder::new().name("hal-xr-chains".into()).spawn(move || {
            let session = session;
            let (a, b, targets) = stereo_chains(&rt.xr, &dev, session.0, format, eye, blit, linear)?;
            log(&format!("projection chains at {}x{} px per eye built aside in {:.1} ms", eye[0], eye[1], started.elapsed().as_secs_f64() * 1e3));
            Ok(ChainSet { size: eye, a, b, targets, unused_since: Instant::now() })
        }) {
            Ok(h) => self.chain_build = Some((to, h)),
            Err(e) => xr_log!("projection chains thread: {e}"),
        }
    }

    fn apply_eye_percent(&mut self, to: u32, xr_window: Option<(u32, Id)>, period: i64) {
        let from = EYE_PERCENT.swap(to, Ordering::AcqRel);
        if from == to { return; }
        let eye = eye_at(to);
        xr_log!("eye scale {from}% -> {to}% ({}x{} px per eye; app GPU {:.2} ms of a {:.2} ms period)", eye[0], eye[1], EYE_GPU_US.load(Ordering::Acquire) as f64 / 1000.0, period as f64 / 1e6);
        if let Some((sid, id)) = xr_window {
            Shared::global().emit_to(sid, id, clock_ns(), |q, id| q.geom(id, 0., 0., 2. * eye[0] as f64, eye[1] as f64, 1., window_state::RESTORE));
        }
    }

    /// Spare chains beyond the newest one, unused for a second, destroyed on a thread.
    fn retire_spare_chains(&mut self) {
        if self.chain_spare.len() <= 1 { return; }
        self.chain_spare.sort_by_key(|c| std::cmp::Reverse(c.unused_since));
        let old: Vec<ChainSet> = self.chain_spare.drain(1..).collect();
        let (keep, gone): (Vec<ChainSet>, Vec<ChainSet>) = old.into_iter().partition(|c| c.unused_since.elapsed() < Duration::from_secs(1));
        self.chain_spare.extend(keep);
        if gone.is_empty() { return; }
        let (rt, dev) = (self.rt.clone(), self.dev.clone());
        let _ = std::thread::Builder::new().name("hal-xr-chains".into()).spawn(move || {
            for mut set in gone { set.destroy(&rt.xr, &dev); }
        });
    }

    /// The projection chains switched to `eye` (a frame of that size arrived: the eye scale
    /// changed), with no image held: a spare set of that size (built aside, `step_eye_scale`),
    /// else made here. The chains left become a spare set (never destroyed under the frame
    /// the compositor may still show). The frame's foveation is applied again.
    fn resize_stereo_chains(&mut self, eye: [u32; 2]) -> Result<(), String> {
        let set = match self.chain_spare.iter().position(|c| c.size == eye) {
            Some(i) => self.chain_spare.swap_remove(i),
            None => {
                crate::profile::count("xr projection chains made in the frame");
                let linear = LINEAR_ACTIVE.load(Ordering::Acquire);
                let (a, b, targets) = stereo_chains(&self.rt.xr, &self.dev, self.handle, self.stereo_base_format, eye, self.stereo_blit, linear)?;
                ChainSet { size: eye, a, b, targets, unused_since: Instant::now() }
            }
        };
        let old_targets = lock(&self.frames.stereo_targets).replace(set.targets);
        let old_a = self.stereo.replace(set.a);
        let old_b = self.stereo_b.replace(set.b);
        if let (Some(a), Some(b), Some(targets)) = (old_a, old_b, old_targets) {
            self.chain_spare.push(ChainSet { size: a.size, a, b, targets, unused_since: Instant::now() });
        }
        self.stereo_shown_chain = None;
        self.foveation_applied = u32::MAX;
        self.apply_foveation();
        Ok(())
    }

    /// The environment depth provider stopped or started as asked (`hal_xr_environment_depth`),
    /// when it changed: stopped, no depth images (no occlusion, no room field).
    fn apply_environment_depth(&mut self) {
        let on = ENVIRONMENT_DEPTH.load(Ordering::Acquire);
        if on == self.environment_depth || self.depth_provider.0 == 0 { return; }
        self.environment_depth = on;
        let xr = &self.rt.xr;
        let r = if on { unsafe { (xr.xrStartEnvironmentDepthProviderMETA)(self.depth_provider) } } else { unsafe { (xr.xrStopEnvironmentDepthProviderMETA)(self.depth_provider) } };
        match r.to_result("environment depth provider") {
            Ok(()) => xr_log!("environment depth provider {}", if on { "started" } else { "stopped" }),
            Err(e) => xr_log!("{e}"),
        }
    }

    /// Simultaneous hands and controllers tracking paused or resumed as asked
    /// (`hal_xr_hands_and_controllers`), when it changed.
    fn apply_hands_and_controllers(&mut self) {
        let on = HANDS_AND_CONTROLLERS.load(Ordering::Acquire);
        if on == self.hands_and_controllers { return; }
        self.hands_and_controllers = on;
        let xr = &self.rt.xr;
        let r = if on {
            unsafe { (xr.xrResumeSimultaneousHandsAndControllersTrackingMETA)(self.handle, &XrSimultaneousHandsAndControllersTrackingResumeInfoMETA::default()) }
        } else {
            unsafe { (xr.xrPauseSimultaneousHandsAndControllersTrackingMETA)(self.handle, &XrSimultaneousHandsAndControllersTrackingPauseInfoMETA::default()) }
        };
        match r.to_result("simultaneous hands and controllers") {
            Ok(()) => xr_log!("simultaneous hands and controllers tracking {}", if on { "resumed" } else { "paused (the hand tracker's own rate)" }),
            Err(e) => xr_log!("{e}"),
        }
    }

    /// The fixed foveation level asked (`hal_xr_foveation`) on both projection chains, when it
    /// changed (or the chains were made anew). A chain made without density maps takes none.
    fn apply_foveation(&mut self) {
        let level = FOVEATION.load(Ordering::Acquire);
        if level == self.foveation_applied { return; }
        self.foveation_applied = level;
        let fb = match level { 0 => XrFoveationLevelFB::NONE, 1 => XrFoveationLevelFB::LOW, 2 => XrFoveationLevelFB::MEDIUM, _ => XrFoveationLevelFB::HIGH };
        for chain in [self.stereo.as_ref(), self.stereo_b.as_ref()].into_iter().flatten() {
            if chain.densities.is_empty() { continue; }
            match vulkan::try_enable_fixed_foveation(&self.rt.xr, self.handle, chain.handle, fb) {
                Ok(()) => xr_log!("projection chain foveation level {level}"),
                Err(e) => xr_log!("projection chain foveation: {e}"),
            }
        }
    }

    /// Copies the newest panel frame into the panel's next image; the frame's `presented`.
    fn update_panel(&mut self) -> Result<Option<Box<dyn FnOnce(u64) + Send>>, String> {
        let Some((image, done, _)) = lock(&self.frames.panel).take() else { return Ok(None) };
        let size = [image.desc.w.min(PANEL_PX[0]), image.desc.h.min(PANEL_PX[1])];
        let src = image.clone();
        let staging = self.panel.staging_for(&self.dev, &image)?;
        self.panel.write(&self.rt.xr, &self.dev, Some(image), |dev, cb, dst| copy_frame(dev, cb, &src, dst, size, 1, staging.as_deref()))?;
        if self.panel_shown.is_none() { xr_log!("first frame on the panel ({}x{} px)", size[0], size[1]); }
        self.panel_shown = Some(size);
        Ok(Some(done))
    }

    /// The head and both eyes at `time` in the local space (the old `begin_frame`: the views
    /// located in the head space, then put in the local space through the head's pose).
    fn locate(&self, time: XrTime) -> Option<(XrPosef, [Eye; 2])> {
        locate_views(&self.rt.xr, self.handle, self.local_space, self.head_space, time)
    }

    /// The stereo window: asks for its frame of this display frame (when none is in flight),
    /// waits for it until shortly before the display time, and copies a frame that came into
    /// the projection chain (an eye per layer) with the eyes it was drawn with.
    fn update_stereo(&mut self, sid: u32, id: Id, time: XrTime, period: i64) -> Result<Option<Box<dyn FnOnce(u64) + Send>>, String> {
        // The XR window draws one frame per XrFrame it is sent, one in flight at a time: this
        // display frame's, asked after xrBeginFrame, taken when it is on the GPU's queue (the
        // runtime waits for that work itself), so the app needs only its CPU time before the
        // deadline. Asked a display frame ahead (the frame before), the app rendered into the
        // chain one image too early: every frame waited ~8 ms for a free image and a third of
        // them came late. A frame still in flight from the loop before is taken when it comes
        // (it shows with its own eyes); one that never came (its batch dropped) is asked again
        // after STEREO_REASK.
        if self.stereo_asked.is_some() && self.stereo_asked_at.elapsed() > STEREO_REASK { self.stereo_asked = None; }
        if self.stereo_asked.is_none() && !self.ask_stereo(sid, id, time, period) {
            return Ok(None);
        }
        // wait for the frame asked for (it is drawn with the eyes of this display frame)
        let started = Instant::now();
        // at most 0.8 of a display period: a frame that is not there keeps the last one showing,
        // and the loop keeps the display rate (waiting to the display time halved it and worse)
        let deadline = Duration::from_nanos((time.as_nanos() - STEREO_MARGIN_NS - clock_ns() as i64).clamp(0, period * 4 / 5) as u64);
        let mut slot = lock(&self.frames.stereo);
        // (elapsed is read once per turn: read twice it can pass the deadline in between, and
        // the subtraction panicked the XR thread)
        loop {
            let left = deadline.saturating_sub(started.elapsed());
            if slot.is_some() || left.is_zero() { break; }
            slot = self.frames.arrived.wait_timeout(slot, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        let Some((image, done, eyes)) = slot.take() else { self.stats[2] += 1; return Ok(None) };
        drop(slot);
        {
            let (asked, scheduled, arrived) = (self.stereo_asked_ns, STEREO_SCHEDULED_NS.load(Ordering::Acquire), STEREO_ARRIVED_NS.load(Ordering::Acquire));
            let us = |a: u64, b: u64| (b.saturating_sub(a) / 1000) as u32;
            let budget = (started + deadline).duration_since(self.stereo_asked_at).as_micros() as u32;
            self.frame_us.push([us(asked, scheduled), us(scheduled, arrived), us(asked, arrived), budget]);
        }
        self.stats[1] += 1;
        self.stats[3] += started.elapsed().as_nanos() as u64;
        // a frame of another size than the chains (the eye scale changed; rendered aside): the
        // chains are made anew at its size, it is copied in
        {
            let eye = [if image.desc.depth >= 2 { image.desc.w } else { image.desc.w / 2 }, image.desc.h];
            let free = lock(&self.frames.stereo_targets).as_ref().is_none_or(|t| t.acquired.is_empty() && t.chain_of(&image).is_none());
            if free && self.stereo.as_ref().is_some_and(|c| c.size != eye) {
                let resized_at = Instant::now();
                match self.resize_stereo_chains(eye) {
                    Ok(()) => xr_log!("projection chains remade at {}x{} px per eye in {:.1} ms", eye[0], eye[1], resized_at.elapsed().as_secs_f64() * 1e3),
                    Err(e) => xr_log!("projection chains at {}x{}: {e}", eye[0], eye[1]),
                }
            }
        }
        let chain = self.stereo.as_mut().expect("stereo chain");
        let copy_at = Instant::now();
        let mut targets = lock(&self.frames.stereo_targets);
        // a frame rendered into the chain: released (with the older ones), no copy
        let in_chain = targets.as_ref().and_then(|t| t.chain_of(&image));
        let size = match targets.as_mut().and_then(|t| t.release_through(&self.rt.xr, &image)) {
            Some(released) => { released?; self.paths[0] += 1; self.stereo_shown_chain = in_chain; chain.size }
            // a frame rendered aside (the chain's image was held): copied in. A held image of an
            // older frame (one this frame replaced) is released first; one still being drawn
            // (a newer frame) keeps its turn and this frame is dropped
            None if targets.as_ref().is_some_and(|t| !t.acquired.is_empty() && !t.submitted) => {
                crate::profile::count("xr stereo frame dropped (chain images held)");
                self.paths[2] += 1;
                return Ok(Some(done));
            }
            None => {
                if let Some(t) = targets.as_mut().filter(|t| !t.acquired.is_empty()) {
                    let held = t.images[*t.acquired.back().unwrap()].clone();
                    t.release_through(&self.rt.xr, &held).unwrap_or(Ok(()))?;
                }
                self.paths[1] += 1;
                self.stereo_shown_chain = Some(chain.handle);
                // a multiview frame: an eye per layer; else both side by side
                let eye_w = if image.desc.depth >= 2 { image.desc.w } else { image.desc.w / 2 };
                let size = [eye_w.min(chain.size[0]), image.desc.h.min(chain.size[1])];
                let src = image.clone();
                let staging = chain.staging_for(&self.dev, &image)?;
                chain.write(&self.rt.xr, &self.dev, Some(image), |dev, cb, dst| copy_frame(dev, cb, &src, dst, size, 2, staging.as_deref()))?;
                size
            }
        };
        drop(targets);
        self.phases[5] += copy_at.elapsed().as_nanos() as u64;
        if self.stereo_shown.is_none() { xr_log!("first stereo frame ({}x{} px per eye)", size[0], size[1]); }
        // the eyes it was drawn for: its own (a streamed picture's render eyes), else its XrFrame's
        let asked = self.stereo_asked.take();
        // (a frame that carries its own eyes far from the ones asked: drawn for another ask, a
        // picture's render eyes long ago, the one-frame jump's suspects; logged)
        if let (Some((own, _)), Some(a)) = (eyes.as_ref(), asked.as_ref()) {
            let (p, q) = (own[0].pose.orientation, a[0].pose.orientation);
            let d = (2.0 * (p.x * q.x + p.y * q.y + p.z * q.z + p.w * q.w).abs().min(1.0).acos()).to_degrees();
            if d > 1.0 { xr_log!("stereo frame shown with eyes {d:.2} deg from the ones asked for its display time"); }
        }
        self.stereo_focus = eyes.map_or(0.0, |(_, f)| f);
        if eyes.is_some() { self.eyes_own += 1 } else { self.eyes_asked += 1 }
        self.stereo_shown = eyes.map(|(e, _)| e).or(asked);
        Ok(Some(done))
    }

    /// Sends the XR window its XrFrame for display time `time` (the head and eyes located at
    /// that time, this display frame's controllers and hands); false when the head is not
    /// tracked.
    fn ask_stereo(&mut self, sid: u32, id: Id, time: XrTime, period: i64) -> bool {
        let shared = Shared::global();
        let Some(frame) = self.frame_at(time) else {
            if self.stats[0] % 360 == 0 { xr_log!("the head is not tracked: no stereo frame asked"); }
            return false;
        };
        lock(&shared.state).pending.remove(&(sid, id));
        let mut bytes = Vec::with_capacity(<XrFrame as crate::buffers::Wire>::SIZE);
        crate::buffers::Wire::write(&frame, &mut bytes);
        let t = time.as_nanos();
        shared.emit_to(sid, id, clock_ns(), |q, id| { q.xr_frame(id, t as u64, &bytes); q.frame_ready(id, (t + period) as u64); });
        let eye = |e: &XrEye| Eye { pose: XrPosef { orientation: makepad_math::Quat { x: e.pose.orientation[0], y: e.pose.orientation[1], z: e.pose.orientation[2], w: e.pose.orientation[3] },
            position: makepad_math::Vec3f { x: e.pose.position[0], y: e.pose.position[1], z: e.pose.position[2] } },
            fov: XrFovf { angle_left: e.fov[0], angle_right: e.fov[1], angle_up: e.fov[2], angle_down: e.fov[3] } };
        self.stereo_asked = Some([eye(&frame.eyes[0]), eye(&frame.eyes[1])]);
        self.stereo_asked_at = Instant::now();
        self.stereo_asked_ns = clock_ns();
        true
    }

    /// The frame for display time `time`: the head and eyes located then, this display frame's
    /// controllers and hands; `None` while the head is not tracked.
    fn frame_at(&self, time: XrTime) -> Option<XrFrame> {
        // (a replay: its tracked path)
        if let Some(r) = track_log::replay() {
            let f = r.tracked_at(time.as_nanos() as u64);
            return Some(XrFrame { controllers: self.controllers, hands: self.hands, ..f });
        }
        let located = self.locate(time);
        let (head, eyes) = located?;
        let fov = |f: &XrFovf| [f.angle_left, f.angle_right, f.angle_up, f.angle_down];
        Some(XrFrame { head: pose(head), eyes: eyes.map(|e| XrEye { pose: pose(e.pose), fov: fov(&e.fov) }), controllers: self.controllers, hands: self.hands })
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
    /// A following panel's pose this frame: in front of the head at its height, `distance` away,
    /// facing it; it stays put while the view's centre is near it and glides back to the centre
    /// (a few tenths of a second) once the view drifts more than ~15 degrees or 30 cm off.
    fn follow_panel(&mut self, time: XrTime, period: u64, distance: f32) -> XrPosef {
        let head = XrSpaceLocation::locate(&self.rt.xr, self.local_space, time, self.head_space).pose;
        let q = head.orientation;
        // the head's forward (0, 0, -1) rotated, flattened to a yaw
        let fx = -2.0 * (q.x * q.z + q.w * q.y);
        let fz = -(1.0 - 2.0 * (q.x * q.x + q.y * q.y));
        let yaw = (-fx).atan2(-fz);
        let target = [head.position.x - yaw.sin() * distance, head.position.y, head.position.z - yaw.cos() * distance];
        let wrap = |a: f32| { let t = std::f32::consts::TAU; let mut d = a % t; if d > std::f32::consts::PI { d -= t; } if d < -std::f32::consts::PI { d += t; } d };
        let f = self.panel_follow.get_or_insert(PanelFollow { pos: target, yaw, moving: false });
        let off = (0..3).map(|i| (target[i] - f.pos[i]).powi(2)).sum::<f32>().sqrt();
        let turn = wrap(yaw - f.yaw).abs();
        if turn > 0.26 || off > 0.3 {
            f.moving = true;
        }
        if f.moving {
            let dt = (period as f32 / 1e9).clamp(0.001, 0.05);
            let k = 1.0 - (-dt * 4.0).exp();
            for i in 0..3 {
                f.pos[i] += (target[i] - f.pos[i]) * k;
            }
            f.yaw += wrap(yaw - f.yaw) * k;
            if turn < 0.02 && off < 0.02 {
                f.moving = false;
            }
        }
        let mut pose = head;
        pose.position.x = f.pos[0];
        pose.position.y = f.pos[1];
        pose.position.z = f.pos[2];
        pose.orientation = Default::default();
        pose.orientation.y = (f.yaw * 0.5).sin();
        pose.orientation.w = (f.yaw * 0.5).cos();
        pose
    }

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
        if let Some(stereo) = &mut self.stereo_b { stereo.destroy(xr, &dev); }
        if let Some((_, build)) = self.chain_build.take() { if let Ok(Ok(set)) = build.join() { self.chain_spare.push(set); } }
        for mut set in self.chain_spare.drain(..) { set.destroy(xr, &dev); }
        if let Some(depth) = &mut self.depth_layer { depth.destroy(xr, &dev); }
        if let Some(depth) = &mut self.depth { depth.destroy(&dev); }
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
        if let Some((thread, stop)) = self.tracker.take() {
            stop.store(true, Ordering::Release);
            let _ = thread.join();
        }
        lock(&self.inputs).0.destroy_input(xr);
        unsafe { (xr.xrDestroySession)(self.handle) }.log_error("xrDestroySession");
    }
}

/// The head and both eyes at `time` in `local_space` (the views located in the head space, then
/// put in the local space through the head's pose).
fn locate_views(xr: &LibOpenXr, session: XrSession, local_space: XrSpace, head_space: XrSpace, time: XrTime) -> Option<(XrPosef, [Eye; 2])> {
    let local_from_head = XrSpaceLocation::locate(xr, local_space, time, head_space);
    let valid = XrSpaceLocationFlags::ORIENTATION_VALID | XrSpaceLocationFlags::POSITION_VALID;
    if !local_from_head.location_flags.contains(valid) { return None; }
    let projection_info = XrViewLocateInfo { view_configuration_type: XrViewConfigurationType::PRIMARY_STEREO, display_time: time, space: head_space, ..Default::default() };
    let mut view_state = XrViewState::default();
    let mut projections = [XrView::default(); 2];
    let mut num_views = 0;
    unsafe { (xr.xrLocateViews)(session, &projection_info, &mut view_state, 2, &mut num_views, &mut projections as *mut _) }.to_result("xrLocateViews").ok()?;
    if num_views != 2 { return None; }
    let eyes = [0, 1].map(|eye| Eye { pose: XrPosef::multiply(&local_from_head.pose, &projections[eye].pose), fov: projections[eye].fov });
    Some((local_from_head.pose, eyes))
}

/// The session's input (actions, hand trackers), polled by the XR thread for its frames, or by
/// the tracking thread while a renderer elsewhere takes the uplink.
struct SharedInputs(CxOpenXrInputs);
unsafe impl Send for SharedInputs {}

/// The tracking thread (while `hal_xr_uplink_next` is called, i.e. a renderer elsewhere takes
/// the poses): once per display frame (woken by the XR thread's xrWaitFrame, `DISPLAY_FRAME`) it
/// polls the input and locates the head, eyes, hands and controllers at a display time of the
/// headset's own grid, the one nearest now + the pose lead (`HalCmd::XrPoseLead`: the measured
/// latency to the display), and publishes them as the uplink pose, tagged with that time. The
/// renderer draws each as it comes: its pictures are on the display's grid and spaced by its
/// period (a renderer pacing itself on its own clock drifted against the display: a picture
/// missing about once a second). Off the XR frame loop's own work; its frames take this
/// thread's newest input meanwhile.
fn track(rt: Arc<Runtime>, inputs: Arc<Mutex<SharedInputs>>, session: XrSession, local_space: XrSpace, head_space: XrSpace, stop: Arc<AtomicBool>) {
    let xr = &rt.xr;
    let (mut samples, mut palm_changes, mut window, mut last_palm) = (0u32, 0u32, Instant::now(), None);
    let mut seen = 0u64;
    // the newest display time logged as tracked (the tracking log); how many display frames
    // past the newest one the poses are located for
    let mut tracked_to = 0i64;
    let mut ahead_frames = 0i64;
    // how far past the runtime's display time the hands are carried at most (ns; debug
    // `debug.xrremote.handlead` ms, read once a second), and when it was read
    let (mut hand_lead_cap, mut hand_lead_at) = (HAND_LEAD_CAP_NS, Instant::now() - Duration::from_secs(2));
    let mut hand_keys = [HandKey::default(); 2];
    // the controllers' angular lead as a share of the stream's lead (percent; debug
    // `debug.xrremote.ctrlang`, read with `handlead`; 255: located at the target as before)
    let mut ctrl_angular = CONTROLLER_ANGULAR_PERCENT;
    // each controller's grip and aim angular velocity of the sample before (rad/s)
    let mut ctrl_last_ang = [[[0f32; 3]; 2]; 2];
    while !stop.load(Ordering::Acquire) {
        // the next display frame (its predicted display time)
        let display = {
            let mut d = lock(&DISPLAY_FRAME);
            while *d == seen && !stop.load(Ordering::Acquire) {
                d = DISPLAY_FRAME_NEW.wait_timeout(d, Duration::from_millis(100)).unwrap_or_else(|e| e.into_inner()).0;
            }
            *d
        };
        if display == seen { continue; }
        seen = display;
        if !uplink_wanted() { continue; }
        if hand_lead_at.elapsed() >= Duration::from_secs(1) {
            hand_lead_at = Instant::now();
            hand_lead_cap = system_property("debug.xrremote.handlead").and_then(|v| v.trim().parse::<i64>().ok()).map_or(HAND_LEAD_CAP_NS, |ms| ms.clamp(0, 100) * 1_000_000);
            ctrl_angular = system_property("debug.xrremote.ctrlang").and_then(|v| v.trim().parse::<u32>().ok()).map_or(CONTROLLER_ANGULAR_PERCENT, |p| p.min(255));
        }
        let period = PERIOD_NS.load(Ordering::Acquire).max(1_000_000);
        // the display time on the headset's grid nearest now + the lead
        let now = clock_ns();
        let want = now as i64 + POSE_LEAD_NS.load(Ordering::Acquire);
        // the display frame this many frames past the newest one: at least now + the lead, and
        // kept while it still is (rounding now + the lead to the nearest frame flipped with the
        // thread's wake jitter when it sat near half a period: the same frame twice, then one
        // skipped, about 2 of 15 pictures a repeat)
        let desired = (want - display as i64) as f64 / period as f64;
        let asked = POSE_FRAMES_AHEAD.load(Ordering::Acquire);
        if asked > 0 {
            ahead_frames = asked;
        } else if ahead_frames < desired.ceil() as i64 || (ahead_frames as f64) > desired + 1.25 {
            ahead_frames = desired.ceil().max(0.0) as i64;
        }
        let target = XrTime::from_nanos(display as i64 + ahead_frames * period);
        // the display times on the grid now 15 ms past, each once: the tracked path (the
        // tracking log's truth for what each display frame should have shown)
        let past = display as i64 + ((now as i64 - 15_000_000 - display as i64) as f64 / period as f64).floor() as i64 * period;
        // (on this display grid: a grid of before, another rate or phase, is left behind)
        for k in (0..8).rev() {
            let t = past - k * period;
            if t <= tracked_to + period / 2 { continue; }
            tracked_to = t;
            let at = XrTime::from_nanos(tracked_to);
            let frame = match track_log::replay() {
                Some(r) => Some(r.tracked_at(tracked_to as u64)),
                None => locate_views(xr, session, local_space, head_space, at).map(|(head, eyes)| {
                    let hands = lock(&inputs).0.locate_hands(xr, session, local_space, at);
                    let fov = |f: &XrFovf| [f.angle_left, f.angle_right, f.angle_up, f.angle_down];
                    XrFrame { head: pose(head), eyes: eyes.map(|e| XrEye { pose: pose(e.pose), fov: fov(&e.fov) }), controllers: Default::default(), hands }
                }),
            };
            if let Some(f) = frame { track_log::push(track_log::TRACKED, tracked_to as u64, now, f); }
        }
        // (a replay: the recorded poses instead of the runtime's)
        let replay = track_log::replay();
        let replayed = replay.as_ref().map(|r| r.sent_at(now, target.as_nanos() as u64));
        let polled = match &replayed {
            Some(f) => Some((f.controllers, f.hands)),
            None => {
                let mut inputs = lock(&inputs);
                let polled = inputs.0.poll(xr, session, local_space, target);
                if polled.is_some() {
                    for (hand, amplitude, duration_us) in std::mem::take(&mut *lock(&HAPTICS)) {
                        inputs.0.apply_haptic(xr, session, hand, amplitude, duration_us);
                    }
                }
                polled.map(|(controllers, mut hands)| {
                    // The hands as the runtime predicts them for its newest display frame (what a
                    // native app draws), interpolated between the tracker's samples and moved
                    // ahead by at most `hand_lead_cap` (`HandKey`). Located at the far
                    // target the joints moved only every 3rd or 4th sample; at the display time
                    // too (68% of samples a repeat: the tracker's ~30 Hz); located at now and
                    // carried rigidly by the grip's angular velocity over ~100 ms they came in
                    // pairs and jumped as each sample landed (2-0-1-0-3 mm with the hand still,
                    // 13-20 cm steps moving).
                    let at_display = inputs.0.locate_hands(xr, session, local_space, XrTime::from_nanos(display as i64));
                    let lead = (target.as_nanos() - display as i64).clamp(0, hand_lead_cap);
                    for h in 0..2 {
                        hands[h] = hand_keys[h].carry(&hands[h], &at_display[h], display as i64, lead);
                    }
                    // The controllers located at the runtime's display time (its short horizon)
                    // and carried to the target: position by the linear velocity, rotation by
                    // the angular velocity over only `ctrl_angular`% of the lead. Located at the
                    // target itself (~55-90 ms out) the runtime's angular prediction overshot
                    // on every sudden stop (a held cube's rotation swung past and back).
                    let mut controllers = controllers;
                    if ctrl_angular != 255 {
                        let moving = inputs.0.locate_controllers(xr, session, local_space, XrTime::from_nanos(display as i64));
                        let lead_s = (target.as_nanos() - display as i64).max(0) as f32 / 1e9;
                        for c in 0..2 {
                            let share = ctrl_angular as f32 / 100.0;
                            // (untracked there: the pose as located at the display time, never the
                            // target's: switching between the two jumped by the runtime's overshoot)
                            let [grip, aim] = [0, 1].map(|k| {
                                let m = moving[c][k];
                                let ang = crate::xr_carry::steadier(m.2, std::mem::replace(&mut ctrl_last_ang[c][k], m.2));
                                crate::xr_carry::carried((m.0, m.1, ang, m.3), lead_s, share).unwrap_or(m.0)
                            });
                            controllers[c].grip_pose = grip;
                            controllers[c].aim_pose = aim;
                        }
                    }
                    (controllers, hands)
                })
            }
        };
        let (controllers, hands) = polled.unwrap_or_default();
        // (how often the tracked hands change between samples: logged every 5 s)
        let palm = hands[1].joints[0].position;
        if last_palm != Some(palm) { palm_changes += 1; last_palm = Some(palm); }
        samples += 1;
        if window.elapsed() >= Duration::from_secs(5) {
            xr_log!("tracking: {:.0} samples/s, right hand changed in {:.0}/s", samples as f64 / window.elapsed().as_secs_f64(), palm_changes as f64 / window.elapsed().as_secs_f64());
            (samples, palm_changes, window) = (0, 0, Instant::now());
        }
        *lock(&TRACKED_INPUT) = Some((controllers, hands));
        let frame = match replayed {
            Some(f) => f,
            None => {
                let Some((head, eyes)) = locate_views(xr, session, local_space, head_space, target) else { continue };
                let fov = |f: &XrFovf| [f.angle_left, f.angle_right, f.angle_up, f.angle_down];
                XrFrame { head: pose(head), eyes: eyes.map(|e| XrEye { pose: pose(e.pose), fov: fov(&e.fov) }), controllers, hands }
            }
        };
        track_log::push(track_log::SENT, now, target.as_nanos() as u64, frame);
        // the hands' grip action space at the target and at now (with velocity): the tracking
        // log's GRIP entries (the held objects' candidate path)
        if replayed.is_none() {
            let mut inputs = lock(&inputs);
            let (at_target, at_now) = (inputs.0.locate_hand_grips(xr, local_space, target), inputs.0.locate_hand_grips(xr, local_space, XrTime::from_nanos(now as i64)));
            // (the joints asked for other times: whether the runtime extrapolates them)
            let (joints_now, joints_later) = (inputs.0.locate_hands(xr, session, local_space, XrTime::from_nanos(now as i64)), inputs.0.locate_hands(xr, session, local_space, XrTime::from_nanos(target.as_nanos() + 40_000_000)));
            let joints_display = inputs.0.locate_hands(xr, session, local_space, XrTime::from_nanos(display as i64));
            drop(inputs);
            let mut g = XrFrame::default();
            for h in 0..2 {
                g.controllers[h].grip_pose = at_target[h].0;
                g.controllers[h].aim_pose = at_now[h].0;
                g.hands[h].joints[0].position = at_now[h].1;
                g.hands[h].joints[1].position = at_now[h].2;
                g.hands[h].flags = at_now[h].3;
                g.hands[h].tips_active = at_target[h].3;
                g.hands[h].joints[2].position = joints_now[h].joints[0].position;
                g.hands[h].joints[3].position = joints_later[h].joints[0].position;
                g.hands[h].joints[4].position = joints_display[h].joints[0].position;
            }
            track_log::push(track_log::GRIP, now, target.as_nanos() as u64, g);
        }
        let mut u = lock(&UPLINK);
        u.1.clear();
        crate::buffers::Wire::write(&frame, &mut u.1);
        u.0 = target.as_nanos() as u64;
        drop(u);
        UPLINK_NEW.notify_all();
    }
}

/// The share of the stream's lead past the runtime's display time a controller's rotation is
/// carried by its angular velocity (percent; the position takes the whole lead): half damps the
/// overshoot on a sudden stop while still leading a steady turn.
const CONTROLLER_ANGULAR_PERCENT: u32 = 50;

/// The longest the uplink's hands are carried past the runtime's display time (`HandKey`): the
/// rest of the stream's lead shows as lag, not jumps.
const HAND_LEAD_CAP_NS: i64 = 50_000_000;

/// One hand's uplink smoothing. The runtime's joints change only with its tracker (25 Hz with
/// hands and controllers tracked at once: located for its display time, 59-68% of display
/// frames repeat the sample before), so the hand sent is: interpolated from where it was sent
/// last towards the newest sample over the usual interval between samples (`from`, `key`: an
/// even step per display frame, no jump when a sample lands early or late), moved ahead by the
/// palm's velocity smoothed over display frames for the lead (position only), then the palm
/// low-passed with a speed-adaptive cutoff (`palm`, `palm_speed`: the 1-euro filter, the
/// whole hand moved by its correction). A sample 25 cm off the one before is taken only when
/// the next agrees (a one-sample 73 cm glitch was seen). Recorded hand motion, frame-to-frame
/// change of the palm's step (mean): 7.2 mm as located, 7.0 interpolated from the sample
/// before (build 24), 3.6 so.
#[derive(Clone, Copy, Default)]
struct HandKey {
    from: Option<XrHand>, key: Option<(XrHand, i64)>, outlier: Option<[f32; 3]>,
    intervals: [i64; 8], n: usize, key_velocity: [f32; 3], velocity: [f32; 3], sent: Option<XrHand>, last_display: i64,
    palm: Option<[f32; 3]>, palm_speed: [f32; 3],
}

impl HandKey {
    const OUTLIER_M: f32 = 0.25;
    /// The 1-euro filter's cutoff: `MIN_CUTOFF_HZ` + `BETA` x the palm's speed (m/s).
    const MIN_CUTOFF_HZ: f32 = 6.0;
    const BETA: f32 = 30.0;

    /// The uplink's hand moved `lead_ns` ahead: `polled` (located at the target: flags, pinches
    /// and grab) with the joints, tips and aim smoothed as above; `located` this display frame's
    /// (`display`). Untracked: `polled`, the history dropped.
    fn carry(&mut self, polled: &XrHand, located: &XrHand, display: i64, lead_ns: i64) -> XrHand {
        if located.flags & xr_hand::IN_VIEW == 0 {
            *self = HandKey::default();
            return *polled;
        }
        let palm_of = |h: &XrHand| h.joints[xr_hand::CENTER].position;
        let dist = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        let p = palm_of(located);
        let new = match self.key {
            None => true,
            Some((k, _)) if palm_of(&k) == p => false,
            // (far off the sample before: only once the next one agrees)
            Some((k, _)) if dist(palm_of(&k), p) > Self::OUTLIER_M && self.outlier.is_none_or(|o| dist(o, p) > Self::OUTLIER_M) => { self.outlier = Some(p); false }
            Some(_) => true,
        };
        if new {
            self.outlier = None;
            if let Some((k, at)) = self.key {
                let dt = display - at;
                if dt >= 150_000_000 {
                    *self = HandKey::default();
                } else {
                    self.intervals[self.n % 8] = dt;
                    self.n += 1;
                    let kp = palm_of(&k);
                    self.key_velocity = std::array::from_fn(|i| (p[i] - kp[i]) / dt.max(1) as f32);
                }
            }
            // (from where the hand was sent last, without its lead and filter: the base)
            self.from = self.sent.or(Some(*located));
            self.key = Some((*located, display));
        }
        let (Some(from), Some((key, at))) = (self.from, self.key) else { return *polled };
        let mut iv: Vec<i64> = self.intervals[..self.n.min(8)].to_vec();
        iv.sort_unstable();
        let interval = iv.get(iv.len() / 2).copied().unwrap_or(40_000_000).max(1);
        let f = ((display - at) as f32 / interval as f32).clamp(0.0, 1.0);
        let lerp = |a: &XrPose, b: &XrPose, d: [f32; 3]| {
            let dot: f32 = (0..4).map(|i| a.orientation[i] * b.orientation[i]).sum();
            let s = if dot < 0.0 { -1.0 } else { 1.0 };
            let q: [f32; 4] = std::array::from_fn(|i| a.orientation[i] + (s * b.orientation[i] - a.orientation[i]) * f);
            let n = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
            XrPose { orientation: q.map(|x| x / n), position: std::array::from_fn(|i| a.position[i] + (b.position[i] - a.position[i]) * f + d[i]) }
        };
        let mut base = key;
        for ((o, a), b) in base.joints.iter_mut().zip(from.joints.iter()).zip(key.joints.iter()) { *o = lerp(a, b, [0.0; 3]); }
        base.aim_pose = lerp(&from.aim_pose, &key.aim_pose, [0.0; 3]);
        self.sent = Some(base);
        // the palm's velocity between the last two samples, smoothed per display frame, and the lead
        for i in 0..3 { self.velocity[i] += 0.1 * (self.key_velocity[i] - self.velocity[i]); }
        let lead = self.velocity.map(|v| v * lead_ns as f32);
        // the 1-euro filter on the led palm
        let target: [f32; 3] = std::array::from_fn(|i| palm_of(&base)[i] + lead[i]);
        let dt_s = ((display - self.last_display) as f32 / 1e9).clamp(0.001, 0.05);
        self.last_display = display;
        let alpha = |cutoff: f32| { let tau = 1.0 / (2.0 * std::f32::consts::PI * cutoff); 1.0 / (1.0 + tau / dt_s) };
        let filtered = match self.palm {
            None => target,
            Some(prev) => {
                let a_d = alpha(1.0);
                for i in 0..3 { self.palm_speed[i] += a_d * ((target[i] - prev[i]) / dt_s - self.palm_speed[i]); }
                let speed = (self.palm_speed.iter().map(|v| v * v).sum::<f32>()).sqrt();
                let a = alpha(Self::MIN_CUTOFF_HZ + Self::BETA * speed);
                std::array::from_fn(|i| prev[i] + a * (target[i] - prev[i]))
            }
        };
        self.palm = Some(filtered);
        let d: [f32; 3] = std::array::from_fn(|i| filtered[i] - palm_of(&base)[i]);
        let moved = |p: &XrPose| XrPose { orientation: p.orientation, position: [p.position[0] + d[0], p.position[1] + d[1], p.position[2] + d[2]] };
        let mut out = *polled;
        for (o, j) in out.joints.iter_mut().zip(base.joints.iter()) { *o = moved(j); }
        out.aim_pose = moved(&base.aim_pose);
        out.tips = key.tips;
        out.tips_active = (key.tips_active & !xr_hand::GRAB_ACTIVE) | (polled.tips_active & xr_hand::GRAB_ACTIVE);
        out.flags = polled.flags | (key.flags & xr_hand::IN_VIEW);
        out
    }
}

/// An Android system property (`adb shell setprop`), if set.
pub(crate) fn system_property(name: &str) -> Option<String> {
    extern "C" { fn __system_property_get(name: *const std::ffi::c_char, value: *mut std::ffi::c_char) -> i32; }
    let name = std::ffi::CString::new(name).ok()?;
    let mut value = [0 as std::ffi::c_char; 92];
    let n = unsafe { __system_property_get(name.as_ptr(), value.as_mut_ptr()) };
    (n > 0).then(|| value[..n as usize].iter().map(|c| *c as u8 as char).collect())
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
