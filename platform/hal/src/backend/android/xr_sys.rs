//! The OpenXR 1.0 C ABI this backend uses (core + XR_KHR_android_create_instance,
//! XR_KHR_loader_init_android, XR_KHR_vulkan_enable2, XR_KHR_android_thread_settings), from the
//! Khronos registry (openxr.h). Entry points are looked up through `xrGetInstanceProcAddr` of
//! the in-APK loader (`libopenxr_loader.so`, only in a Quest build).
#![allow(non_camel_case_types, non_snake_case, dead_code)]
use std::ffi::{c_char, c_void, CStr};

pub type XrResult = i32;
pub type XrTime = i64;
pub type XrPath = u64;
pub type XrSystemId = u64;
pub type XrInstance = u64;
pub type XrSession = u64;
pub type XrSpace = u64;
pub type XrSwapchain = u64;
pub type XrActionSet = u64;
pub type XrAction = u64;
pub type XrBool32 = u32;

pub const SUCCESS: XrResult = 0;
pub const SESSION_LOSS_PENDING: XrResult = 3;
pub const EVENT_UNAVAILABLE: XrResult = 4;
pub const FRAME_DISCARDED: XrResult = 9;
pub const ERROR_SESSION_LOST: XrResult = -17;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ty(pub i32);
impl Ty {
    pub const EXTENSION_PROPERTIES: Ty = Ty(2);
    pub const INSTANCE_CREATE_INFO: Ty = Ty(3);
    pub const SYSTEM_GET_INFO: Ty = Ty(4);
    pub const SESSION_CREATE_INFO: Ty = Ty(8);
    pub const SWAPCHAIN_CREATE_INFO: Ty = Ty(9);
    pub const SESSION_BEGIN_INFO: Ty = Ty(10);
    pub const FRAME_END_INFO: Ty = Ty(12);
    pub const EVENT_DATA_BUFFER: Ty = Ty(16);
    pub const EVENT_DATA_INSTANCE_LOSS_PENDING: Ty = Ty(17);
    pub const EVENT_DATA_SESSION_STATE_CHANGED: Ty = Ty(18);
    pub const ACTION_STATE_BOOLEAN: Ty = Ty(23);
    pub const ACTION_STATE_VECTOR2F: Ty = Ty(25);
    pub const ACTION_SET_CREATE_INFO: Ty = Ty(28);
    pub const ACTION_CREATE_INFO: Ty = Ty(29);
    pub const FRAME_WAIT_INFO: Ty = Ty(33);
    pub const COMPOSITION_LAYER_QUAD: Ty = Ty(36);
    pub const REFERENCE_SPACE_CREATE_INFO: Ty = Ty(37);
    pub const ACTION_SPACE_CREATE_INFO: Ty = Ty(38);
    pub const SPACE_LOCATION: Ty = Ty(42);
    pub const FRAME_STATE: Ty = Ty(44);
    pub const FRAME_BEGIN_INFO: Ty = Ty(46);
    pub const INTERACTION_PROFILE_SUGGESTED_BINDING: Ty = Ty(51);
    pub const SWAPCHAIN_IMAGE_ACQUIRE_INFO: Ty = Ty(55);
    pub const SWAPCHAIN_IMAGE_WAIT_INFO: Ty = Ty(56);
    pub const SWAPCHAIN_IMAGE_RELEASE_INFO: Ty = Ty(57);
    pub const ACTION_STATE_GET_INFO: Ty = Ty(58);
    pub const SESSION_ACTION_SETS_ATTACH_INFO: Ty = Ty(60);
    pub const ACTIONS_SYNC_INFO: Ty = Ty(61);
    pub const INSTANCE_CREATE_INFO_ANDROID_KHR: Ty = Ty(1000008000);
    pub const GRAPHICS_BINDING_VULKAN_KHR: Ty = Ty(1000025000);
    pub const SWAPCHAIN_IMAGE_VULKAN_KHR: Ty = Ty(1000025001);
    pub const GRAPHICS_REQUIREMENTS_VULKAN_KHR: Ty = Ty(1000025002);
    pub const LOADER_INIT_INFO_ANDROID_KHR: Ty = Ty(1000089000);
    pub const VULKAN_INSTANCE_CREATE_INFO_KHR: Ty = Ty(1000090000);
    pub const VULKAN_DEVICE_CREATE_INFO_KHR: Ty = Ty(1000090001);
    pub const VULKAN_GRAPHICS_DEVICE_GET_INFO_KHR: Ty = Ty(1000090003);
}

pub const FORM_FACTOR_HEAD_MOUNTED_DISPLAY: i32 = 1;
pub const VIEW_CONFIGURATION_PRIMARY_STEREO: i32 = 2;
pub const REFERENCE_SPACE_LOCAL: i32 = 2;
pub const ENVIRONMENT_BLEND_MODE_OPAQUE: i32 = 1;
pub const ACTION_TYPE_BOOLEAN_INPUT: i32 = 1;
pub const ACTION_TYPE_VECTOR2F_INPUT: i32 = 3;
pub const ACTION_TYPE_POSE_INPUT: i32 = 4;
pub const EYE_VISIBILITY_BOTH: i32 = 0;
pub const ANDROID_THREAD_TYPE_RENDERER_MAIN: i32 = 3;

pub const SWAPCHAIN_USAGE_COLOR_ATTACHMENT: u64 = 0x1;
pub const SWAPCHAIN_USAGE_TRANSFER_DST: u64 = 0x10;
pub const SWAPCHAIN_USAGE_SAMPLED: u64 = 0x20;
pub const SWAPCHAIN_CREATE_STATIC_IMAGE: u64 = 0x2;
pub const LAYER_BLEND_TEXTURE_SOURCE_ALPHA: u64 = 0x2;
pub const SPACE_LOCATION_ORIENTATION_VALID: u64 = 0x1;
pub const SPACE_LOCATION_POSITION_VALID: u64 = 0x2;

pub mod session_state {
    pub const IDLE: i32 = 1;
    pub const READY: i32 = 2;
    pub const SYNCHRONIZED: i32 = 3;
    pub const VISIBLE: i32 = 4;
    pub const FOCUSED: i32 = 5;
    pub const STOPPING: i32 = 6;
    pub const LOSS_PENDING: i32 = 7;
    pub const EXITING: i32 = 8;
}

/// Fixed-size C strings of OpenXR structs.
pub fn fixed<const N: usize>(s: &str) -> [c_char; N] {
    let mut out = [0 as c_char; N];
    for (o, b) in out.iter_mut().zip(s.bytes().take(N - 1)) { *o = b as c_char; }
    out
}

#[repr(C)] #[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quat { pub x: f32, pub y: f32, pub z: f32, pub w: f32 }
#[repr(C)] #[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 { pub x: f32, pub y: f32, pub z: f32 }
#[repr(C)] #[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 { pub x: f32, pub y: f32 }
#[repr(C)] #[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Posef { pub orientation: Quat, pub position: Vec3 }
impl Posef {
    pub const IDENTITY: Posef = Posef { orientation: Quat { x: 0., y: 0., z: 0., w: 1. }, position: Vec3 { x: 0., y: 0., z: 0. } };
}
#[repr(C)] #[derive(Clone, Copy, Debug, Default)]
pub struct Offset2Di { pub x: i32, pub y: i32 }
#[repr(C)] #[derive(Clone, Copy, Debug, Default)]
pub struct Extent2Di { pub width: i32, pub height: i32 }
#[repr(C)] #[derive(Clone, Copy, Debug, Default)]
pub struct Rect2Di { pub offset: Offset2Di, pub extent: Extent2Di }
#[repr(C)] #[derive(Clone, Copy, Debug, Default)]
pub struct Extent2Df { pub width: f32, pub height: f32 }

#[repr(C)]
pub struct ApplicationInfo { pub application_name: [c_char; 128], pub application_version: u32, pub engine_name: [c_char; 128], pub engine_version: u32, pub api_version: u64 }
#[repr(C)]
pub struct InstanceCreateInfo { pub ty: Ty, pub next: *const c_void, pub create_flags: u64, pub application_info: ApplicationInfo,
    pub enabled_api_layer_count: u32, pub enabled_api_layer_names: *const *const c_char, pub enabled_extension_count: u32, pub enabled_extension_names: *const *const c_char }
#[repr(C)]
pub struct InstanceCreateInfoAndroidKHR { pub ty: Ty, pub next: *const c_void, pub application_vm: *mut c_void, pub application_activity: *mut c_void }
#[repr(C)]
pub struct LoaderInitInfoAndroidKHR { pub ty: Ty, pub next: *const c_void, pub application_vm: *mut c_void, pub application_context: *mut c_void }
#[repr(C)]
pub struct ExtensionProperties { pub ty: Ty, pub next: *mut c_void, pub extension_name: [c_char; 128], pub extension_version: u32 }
impl ExtensionProperties {
    pub fn empty() -> Self { Self { ty: Ty::EXTENSION_PROPERTIES, next: std::ptr::null_mut(), extension_name: [0; 128], extension_version: 0 } }
    pub fn name(&self) -> &CStr { unsafe { CStr::from_ptr(self.extension_name.as_ptr()) } }
}
#[repr(C)]
pub struct SystemGetInfo { pub ty: Ty, pub next: *const c_void, pub form_factor: i32 }
#[repr(C)]
pub struct GraphicsRequirementsVulkanKHR { pub ty: Ty, pub next: *mut c_void, pub min_api_version_supported: u64, pub max_api_version_supported: u64 }
#[repr(C)]
pub struct VulkanInstanceCreateInfoKHR { pub ty: Ty, pub next: *const c_void, pub system_id: XrSystemId, pub create_flags: u64,
    pub pfn_get_instance_proc_addr: *const c_void, pub vulkan_create_info: *const c_void, pub vulkan_allocator: *const c_void }
#[repr(C)]
pub struct VulkanDeviceCreateInfoKHR { pub ty: Ty, pub next: *const c_void, pub system_id: XrSystemId, pub create_flags: u64,
    pub pfn_get_instance_proc_addr: *const c_void, pub vulkan_physical_device: usize, pub vulkan_create_info: *const c_void, pub vulkan_allocator: *const c_void }
#[repr(C)]
pub struct VulkanGraphicsDeviceGetInfoKHR { pub ty: Ty, pub next: *const c_void, pub system_id: XrSystemId, pub vulkan_instance: usize }
#[repr(C)]
pub struct GraphicsBindingVulkanKHR { pub ty: Ty, pub next: *const c_void, pub instance: usize, pub physical_device: usize, pub device: usize, pub queue_family_index: u32, pub queue_index: u32 }
#[repr(C)]
pub struct SessionCreateInfo { pub ty: Ty, pub next: *const c_void, pub create_flags: u64, pub system_id: XrSystemId }
#[repr(C)]
pub struct SessionBeginInfo { pub ty: Ty, pub next: *const c_void, pub primary_view_configuration_type: i32 }
#[repr(C)]
pub struct EventDataBuffer { pub ty: Ty, pub next: *const c_void, pub varying: [u8; 4000] }
#[repr(C)]
pub struct EventDataSessionStateChanged { pub ty: Ty, pub next: *const c_void, pub session: XrSession, pub state: i32, pub time: XrTime }
#[repr(C)]
pub struct ReferenceSpaceCreateInfo { pub ty: Ty, pub next: *const c_void, pub reference_space_type: i32, pub pose_in_reference_space: Posef }
#[repr(C)]
pub struct SwapchainCreateInfo { pub ty: Ty, pub next: *const c_void, pub create_flags: u64, pub usage_flags: u64, pub format: i64,
    pub sample_count: u32, pub width: u32, pub height: u32, pub face_count: u32, pub array_size: u32, pub mip_count: u32 }
#[repr(C)] #[derive(Clone, Copy)]
pub struct SwapchainImageVulkanKHR { pub ty: Ty, pub next: *mut c_void, pub image: u64 }
#[repr(C)]
pub struct SwapchainImageAcquireInfo { pub ty: Ty, pub next: *const c_void }
#[repr(C)]
pub struct SwapchainImageWaitInfo { pub ty: Ty, pub next: *const c_void, pub timeout: i64 }
#[repr(C)]
pub struct SwapchainImageReleaseInfo { pub ty: Ty, pub next: *const c_void }
#[repr(C)]
pub struct FrameWaitInfo { pub ty: Ty, pub next: *const c_void }
#[repr(C)]
pub struct FrameState { pub ty: Ty, pub next: *mut c_void, pub predicted_display_time: XrTime, pub predicted_display_period: i64, pub should_render: XrBool32 }
#[repr(C)]
pub struct FrameBeginInfo { pub ty: Ty, pub next: *const c_void }
#[repr(C)]
pub struct FrameEndInfo { pub ty: Ty, pub next: *const c_void, pub display_time: XrTime, pub environment_blend_mode: i32, pub layer_count: u32, pub layers: *const *const c_void }
#[repr(C)] #[derive(Clone, Copy)]
pub struct SwapchainSubImage { pub swapchain: XrSwapchain, pub image_rect: Rect2Di, pub image_array_index: u32 }
#[repr(C)]
pub struct CompositionLayerQuad { pub ty: Ty, pub next: *const c_void, pub layer_flags: u64, pub space: XrSpace, pub eye_visibility: i32,
    pub sub_image: SwapchainSubImage, pub pose: Posef, pub size: Extent2Df }
#[repr(C)]
pub struct ActionSetCreateInfo { pub ty: Ty, pub next: *const c_void, pub action_set_name: [c_char; 64], pub localized_action_set_name: [c_char; 128], pub priority: u32 }
#[repr(C)]
pub struct ActionCreateInfo { pub ty: Ty, pub next: *const c_void, pub action_name: [c_char; 64], pub action_type: i32,
    pub count_subaction_paths: u32, pub subaction_paths: *const XrPath, pub localized_action_name: [c_char; 128] }
#[repr(C)] #[derive(Clone, Copy)]
pub struct ActionSuggestedBinding { pub action: XrAction, pub binding: XrPath }
#[repr(C)]
pub struct InteractionProfileSuggestedBinding { pub ty: Ty, pub next: *const c_void, pub interaction_profile: XrPath, pub count_suggested_bindings: u32, pub suggested_bindings: *const ActionSuggestedBinding }
#[repr(C)]
pub struct SessionActionSetsAttachInfo { pub ty: Ty, pub next: *const c_void, pub count_action_sets: u32, pub action_sets: *const XrActionSet }
#[repr(C)]
pub struct ActiveActionSet { pub action_set: XrActionSet, pub subaction_path: XrPath }
#[repr(C)]
pub struct ActionsSyncInfo { pub ty: Ty, pub next: *const c_void, pub count_active_action_sets: u32, pub active_action_sets: *const ActiveActionSet }
#[repr(C)]
pub struct ActionStateGetInfo { pub ty: Ty, pub next: *const c_void, pub action: XrAction, pub subaction_path: XrPath }
#[repr(C)]
pub struct ActionStateBoolean { pub ty: Ty, pub next: *mut c_void, pub current_state: XrBool32, pub changed_since_last_sync: XrBool32, pub last_change_time: XrTime, pub is_active: XrBool32 }
#[repr(C)]
pub struct ActionStateVector2f { pub ty: Ty, pub next: *mut c_void, pub current_state: Vec2, pub changed_since_last_sync: XrBool32, pub last_change_time: XrTime, pub is_active: XrBool32 }
#[repr(C)]
pub struct ActionSpaceCreateInfo { pub ty: Ty, pub next: *const c_void, pub action: XrAction, pub subaction_path: XrPath, pub pose_in_action_space: Posef }
#[repr(C)]
pub struct SpaceLocation { pub ty: Ty, pub next: *mut c_void, pub location_flags: u64, pub pose: Posef }

type Gipa = unsafe extern "system" fn(XrInstance, *const c_char, *mut Option<unsafe extern "system" fn()>) -> XrResult;

macro_rules! xr_fns {
    ($($name:ident: fn($($arg:ty),*);)*) => {
        /// The instance's entry points (`xrGetInstanceProcAddr` per name).
        pub struct Xr {
            pub instance: XrInstance,
            $(pub $name: unsafe extern "system" fn($($arg),*) -> XrResult,)*
            pub xrSetAndroidApplicationThreadKHR: Option<unsafe extern "system" fn(XrSession, i32, u32) -> XrResult>,
        }
        impl Xr {
            #[allow(non_snake_case)]
            unsafe fn load(gipa: Gipa, instance: XrInstance) -> Result<Self, String> {
                Ok(Self { instance, $($name: std::mem::transmute(lookup(gipa, instance, concat!(stringify!($name), "\0"))?),)*
                    xrSetAndroidApplicationThreadKHR: lookup(gipa, instance, "xrSetAndroidApplicationThreadKHR\0").ok().map(|f| std::mem::transmute(f)) })
            }
        }
    };
}

unsafe fn lookup(gipa: Gipa, instance: XrInstance, name: &str) -> Result<unsafe extern "system" fn(), String> {
    let mut f = None;
    let r = gipa(instance, name.as_ptr() as *const c_char, &mut f);
    f.filter(|_| r == SUCCESS).ok_or_else(|| format!("{}: {}", name.trim_end_matches('\0'), r))
}

xr_fns! {
    xrDestroyInstance: fn(XrInstance);
    xrGetSystem: fn(XrInstance, *const SystemGetInfo, *mut XrSystemId);
    xrGetVulkanGraphicsRequirements2KHR: fn(XrInstance, XrSystemId, *mut GraphicsRequirementsVulkanKHR);
    xrCreateVulkanInstanceKHR: fn(XrInstance, *const VulkanInstanceCreateInfoKHR, *mut usize, *mut i32);
    xrGetVulkanGraphicsDevice2KHR: fn(XrInstance, *const VulkanGraphicsDeviceGetInfoKHR, *mut usize);
    xrCreateVulkanDeviceKHR: fn(XrInstance, *const VulkanDeviceCreateInfoKHR, *mut usize, *mut i32);
    xrCreateSession: fn(XrInstance, *const SessionCreateInfo, *mut XrSession);
    xrPollEvent: fn(XrInstance, *mut EventDataBuffer);
    xrBeginSession: fn(XrSession, *const SessionBeginInfo);
    xrEndSession: fn(XrSession);
    xrWaitFrame: fn(XrSession, *const FrameWaitInfo, *mut FrameState);
    xrBeginFrame: fn(XrSession, *const FrameBeginInfo);
    xrEndFrame: fn(XrSession, *const FrameEndInfo);
    xrCreateReferenceSpace: fn(XrSession, *const ReferenceSpaceCreateInfo, *mut XrSpace);
    xrEnumerateSwapchainFormats: fn(XrSession, u32, *mut u32, *mut i64);
    xrCreateSwapchain: fn(XrSession, *const SwapchainCreateInfo, *mut XrSwapchain);
    xrEnumerateSwapchainImages: fn(XrSwapchain, u32, *mut u32, *mut SwapchainImageVulkanKHR);
    xrAcquireSwapchainImage: fn(XrSwapchain, *const SwapchainImageAcquireInfo, *mut u32);
    xrWaitSwapchainImage: fn(XrSwapchain, *const SwapchainImageWaitInfo);
    xrReleaseSwapchainImage: fn(XrSwapchain, *const SwapchainImageReleaseInfo);
    xrStringToPath: fn(XrInstance, *const c_char, *mut XrPath);
    xrCreateActionSet: fn(XrInstance, *const ActionSetCreateInfo, *mut XrActionSet);
    xrCreateAction: fn(XrActionSet, *const ActionCreateInfo, *mut XrAction);
    xrSuggestInteractionProfileBindings: fn(XrInstance, *const InteractionProfileSuggestedBinding);
    xrAttachSessionActionSets: fn(XrSession, *const SessionActionSetsAttachInfo);
    xrSyncActions: fn(XrSession, *const ActionsSyncInfo);
    xrGetActionStateBoolean: fn(XrSession, *const ActionStateGetInfo, *mut ActionStateBoolean);
    xrGetActionStateVector2f: fn(XrSession, *const ActionStateGetInfo, *mut ActionStateVector2f);
    xrCreateActionSpace: fn(XrSession, *const ActionSpaceCreateInfo, *mut XrSpace);
    xrLocateSpace: fn(XrSpace, XrSpace, XrTime, *mut SpaceLocation);
}

#[link(name = "dl")]
extern "C" {
    fn dlopen(name: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

/// The in-APK loader's `xrGetInstanceProcAddr`; `None` in an APK without one (not a Quest build).
/// The library is never closed.
pub fn loader() -> Option<Gipa> {
    static GIPA: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let p = *GIPA.get_or_init(|| unsafe {
        let lib = dlopen(c"libopenxr_loader.so".as_ptr(), 2); // RTLD_NOW
        if lib.is_null() { return 0; }
        dlsym(lib, c"xrGetInstanceProcAddr".as_ptr()) as usize
    });
    (p != 0).then(|| unsafe { std::mem::transmute::<usize, Gipa>(p) })
}

pub fn check(r: XrResult, what: &str) -> Result<(), String> {
    if r >= 0 { Ok(()) } else { Err(format!("{what}: XrResult {r}")) }
}

/// xrInitializeLoaderKHR, then an instance with `extensions` (each must be offered).
/// # Safety
/// `vm` and `activity` are the process's JavaVM and a global reference to the Activity.
/// `extensions`: (name, required); an optional one the runtime lacks is left out. Returns the
/// instance and the extensions it was made with.
pub unsafe fn create_instance(gipa: Gipa, vm: *mut c_void, activity: *mut c_void, extensions: &[(&'static CStr, bool)]) -> Result<(Xr, Vec<&'static CStr>), String> {
    type InitLoader = unsafe extern "system" fn(*const LoaderInitInfoAndroidKHR) -> XrResult;
    type EnumExt = unsafe extern "system" fn(*const c_char, u32, *mut u32, *mut ExtensionProperties) -> XrResult;
    type Create = unsafe extern "system" fn(*const InstanceCreateInfo, *mut XrInstance) -> XrResult;
    let init: InitLoader = std::mem::transmute(lookup(gipa, 0, "xrInitializeLoaderKHR\0")?);
    let info = LoaderInitInfoAndroidKHR { ty: Ty::LOADER_INIT_INFO_ANDROID_KHR, next: std::ptr::null(), application_vm: vm, application_context: activity };
    check(init(&info), "xrInitializeLoaderKHR")?;
    let enumerate: EnumExt = std::mem::transmute(lookup(gipa, 0, "xrEnumerateInstanceExtensionProperties\0")?);
    let mut count = 0;
    check(enumerate(std::ptr::null(), 0, &mut count, std::ptr::null_mut()), "xrEnumerateInstanceExtensionProperties")?;
    let mut offered: Vec<ExtensionProperties> = (0..count).map(|_| ExtensionProperties::empty()).collect();
    check(enumerate(std::ptr::null(), count, &mut count, offered.as_mut_ptr()), "xrEnumerateInstanceExtensionProperties")?;
    offered.truncate(count as usize);
    let has = |e: &CStr| offered.iter().any(|o| o.name() == e);
    if let Some((missing, _)) = extensions.iter().find(|(e, required)| *required && !has(e)) {
        return Err(format!("the OpenXR runtime lacks {}", missing.to_string_lossy()));
    }
    let enabled: Vec<&'static CStr> = extensions.iter().filter(|(e, _)| has(e)).map(|(e, _)| *e).collect();
    let names: Vec<*const c_char> = enabled.iter().map(|e| e.as_ptr()).collect();
    let android = InstanceCreateInfoAndroidKHR { ty: Ty::INSTANCE_CREATE_INFO_ANDROID_KHR, next: std::ptr::null(), application_vm: vm, application_activity: activity };
    let create_info = InstanceCreateInfo { ty: Ty::INSTANCE_CREATE_INFO, next: &android as *const _ as _, create_flags: 0,
        application_info: ApplicationInfo { application_name: fixed("Makepad"), application_version: 1, engine_name: fixed("Makepad HAL"), engine_version: 1,
            api_version: (1 << 48) | 46 }, // XR_MAKE_VERSION(1, 0, 46)
        enabled_api_layer_count: 0, enabled_api_layer_names: std::ptr::null(),
        enabled_extension_count: names.len() as u32, enabled_extension_names: names.as_ptr() };
    let create: Create = std::mem::transmute(lookup(gipa, 0, "xrCreateInstance\0")?);
    let mut instance = 0;
    check(create(&create_info, &mut instance), "xrCreateInstance")?;
    Xr::load(gipa, instance).map(|xr| (xr, enabled)).inspect_err(|_| {
        if let Ok(destroy) = lookup(gipa, instance, "xrDestroyInstance\0") {
            let destroy: unsafe extern "system" fn(XrInstance) -> XrResult = std::mem::transmute(destroy);
            destroy(instance);
        }
    })
}

impl Xr {
    pub fn path(&self, s: &CStr) -> Result<XrPath, String> {
        let mut p = 0;
        check(unsafe { (self.xrStringToPath)(self.instance, s.as_ptr(), &mut p) }, "xrStringToPath")?;
        Ok(p)
    }
}
