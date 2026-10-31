//! OpenXR 1.0 ABI subset used by the Linux runtime. Integer discriminants stay
//! integers at the FFI boundary: an unknown value is an error, not Rust enum UB.
use std::ffi::{c_char, c_void};

pub type Handle = u64;
pub type Time = i64;
pub type Result = i32;
pub type Proc = Option<unsafe extern "system" fn()>;
pub const API_VERSION: u64 = 1 << 48;
pub const SUCCESS: Result = 0;
pub const TIMEOUT: Result = 1;
pub const SESSION_LOSS_PENDING: Result = 3;
pub const EVENT_UNAVAILABLE: Result = 4;
pub const SPACE_BOUNDS_UNAVAILABLE: Result = 7;
pub const SESSION_NOT_FOCUSED: Result = 8;
pub const FRAME_DISCARDED: Result = 9;
pub const VALIDATION: Result = -1;
pub const RUNTIME_FAILURE: Result = -2;
pub const OUT_OF_MEMORY: Result = -3;
pub const API_UNSUPPORTED: Result = -4;
pub const INITIALIZATION_FAILED: Result = -6;
pub const FUNCTION_UNSUPPORTED: Result = -7;
pub const FEATURE_UNSUPPORTED: Result = -8;
pub const EXTENSION_NOT_PRESENT: Result = -9;
pub const SIZE_INSUFFICIENT: Result = -11;
pub const HANDLE_INVALID: Result = -12;
pub const SESSION_RUNNING: Result = -14;
pub const SESSION_NOT_RUNNING: Result = -16;
pub const SESSION_LOST: Result = -17;
pub const SYSTEM_INVALID: Result = -18;
pub const PATH_INVALID: Result = -19;
pub const PATH_COUNT_EXCEEDED: Result = -20;
pub const PATH_FORMAT_INVALID: Result = -21;
pub const PATH_UNSUPPORTED: Result = -22;
pub const LAYER_INVALID: Result = -23;
pub const SWAPCHAIN_RECT_INVALID: Result = -25;
pub const SWAPCHAIN_FORMAT_UNSUPPORTED: Result = -26;
pub const ACTION_TYPE_MISMATCH: Result = -27;
pub const SESSION_NOT_READY: Result = -28;
pub const SESSION_NOT_STOPPING: Result = -29;
pub const TIME_INVALID: Result = -30;
pub const REFERENCE_SPACE_UNSUPPORTED: Result = -31;
pub const FILE_ACCESS_ERROR: Result = -32;
pub const FORM_FACTOR_UNSUPPORTED: Result = -34;
pub const FORM_FACTOR_UNAVAILABLE: Result = -35;
pub const API_LAYER_NOT_PRESENT: Result = -36;
pub const CALL_ORDER_INVALID: Result = -37;
pub const GRAPHICS_DEVICE_INVALID: Result = -38;
pub const POSE_INVALID: Result = -39;
pub const INDEX_OUT_OF_RANGE: Result = -40;
pub const VIEW_CONFIGURATION_UNSUPPORTED: Result = -41;
pub const ENVIRONMENT_BLEND_MODE_UNSUPPORTED: Result = -42;
pub const NAME_DUPLICATED: Result = -44;
pub const NAME_INVALID: Result = -45;
pub const ACTIONSET_NOT_ATTACHED: Result = -46;
pub const ACTIONSETS_ALREADY_ATTACHED: Result = -47;
pub const LOCALIZED_NAME_DUPLICATED: Result = -48;
pub const LOCALIZED_NAME_INVALID: Result = -49;
pub const GRAPHICS_REQUIREMENTS_CALL_MISSING: Result = -50;

macro_rules! record {
    ($name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        #[repr(C)]
        #[derive(Clone, Copy)]
        pub struct $name { $(pub $field: $ty),* }
    };
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose { pub orientation: [f32; 4], pub position: [f32; 3] }
impl Default for Pose {
    fn default() -> Self { Self { orientation: [0., 0., 0., 1.], position: [0.; 3] } }
}
record!(Base { ty: i32, next: *const c_void });
record!(ApplicationInfo { name: [c_char; 128], version: u32, engine: [c_char; 128], engine_version: u32, api_version: u64 });
record!(InstanceCreateInfo { ty: i32, next: *const c_void, flags: u64, app: ApplicationInfo, layer_count: u32, layers: *const *const c_char, extension_count: u32, extensions: *const *const c_char });
record!(InstanceProperties { ty: i32, next: *mut c_void, version: u64, name: [c_char; 128] });
record!(ExtensionProperties { ty: i32, next: *mut c_void, name: [c_char; 128], version: u32 });
record!(ApiLayerProperties { ty: i32, next: *mut c_void, name: [c_char; 256], spec_version: u64, layer_version: u32, description: [c_char; 256] });
record!(SystemGetInfo { ty: i32, next: *const c_void, form_factor: i32 });
record!(GraphicsProperties { max_height: u32, max_width: u32, max_layers: u32 });
record!(TrackingProperties { orientation: u32, position: u32 });
record!(SystemProperties { ty: i32, next: *mut c_void, system: Handle, vendor: u32, name: [c_char; 256], graphics: GraphicsProperties, tracking: TrackingProperties });
record!(SessionCreateInfo { ty: i32, next: *const c_void, flags: u64, system: Handle });
record!(SessionBeginInfo { ty: i32, next: *const c_void, view_type: i32 });
record!(EventBuffer { ty: i32, next: *const c_void, bytes: [u8; 4000] });
record!(EventSessionStateChanged { ty: i32, next: *const c_void, session: Handle, state: i32, time: Time });
record!(EventInteractionProfileChanged { ty: i32, next: *const c_void, session: Handle });
record!(ViewConfigurationProperties { ty: i32, next: *mut c_void, view_type: i32, mutable_fov: u32 });
record!(ViewConfigurationView { ty: i32, next: *mut c_void, recommended_width: u32, max_width: u32, recommended_height: u32, max_height: u32, recommended_samples: u32, max_samples: u32 });
record!(ReferenceSpaceCreateInfo { ty: i32, next: *const c_void, space_type: i32, pose: Pose });
record!(Extent2f { width: f32, height: f32 });
record!(SpaceLocation { ty: i32, next: *mut c_void, flags: u64, pose: Pose });
record!(SpaceVelocity { ty: i32, next: *mut c_void, flags: u64, linear: [f32; 3], angular: [f32; 3] });
record!(ViewLocateInfo { ty: i32, next: *const c_void, view_type: i32, time: Time, space: Handle });
record!(ViewState { ty: i32, next: *mut c_void, flags: u64 });
record!(View { ty: i32, next: *mut c_void, pose: Pose, fov: [f32; 4] });
record!(FrameWaitInfo { ty: i32, next: *const c_void });
record!(FrameState { ty: i32, next: *mut c_void, display_time: Time, period: Time, should_render: u32 });
record!(FrameBeginInfo { ty: i32, next: *const c_void });
record!(FrameEndInfo { ty: i32, next: *const c_void, display_time: Time, blend_mode: i32, layer_count: u32, layers: *const *const CompositionLayer });
record!(Rect2i { offset: [i32; 2], extent: [i32; 2] });
record!(SwapchainSubImage { swapchain: Handle, rect: Rect2i, array_index: u32 });
record!(CompositionLayer { ty: i32, next: *const c_void, flags: u64, space: Handle });
record!(ProjectionView { ty: i32, next: *const c_void, pose: Pose, fov: [f32; 4], image: SwapchainSubImage });
record!(ProjectionLayer { ty: i32, next: *const c_void, flags: u64, space: Handle, view_count: u32, views: *const ProjectionView });
record!(QuadLayer { ty: i32, next: *const c_void, flags: u64, space: Handle, visibility: i32, image: SwapchainSubImage, pose: Pose, size: Extent2f });
record!(DepthInfo { ty: i32, next: *const c_void, image: SwapchainSubImage, min_depth: f32, max_depth: f32, near_z: f32, far_z: f32 });
record!(SwapchainCreateInfo { ty: i32, next: *const c_void, flags: u64, usage: u64, format: i64, samples: u32, width: u32, height: u32, faces: u32, array_size: u32, mip_count: u32 });
record!(SwapchainImage { ty: i32, next: *mut c_void, image: u64 });
record!(SwapchainAcquireInfo { ty: i32, next: *const c_void });
record!(SwapchainWaitInfo { ty: i32, next: *const c_void, timeout: Time });
record!(SwapchainReleaseInfo { ty: i32, next: *const c_void });
record!(ActionSetCreateInfo { ty: i32, next: *const c_void, name: [c_char; 64], localized_name: [c_char; 128], priority: u32 });
record!(ActionCreateInfo { ty: i32, next: *const c_void, name: [c_char; 64], action_type: i32, path_count: u32, paths: *const Handle, localized_name: [c_char; 128] });
record!(ActionSpaceCreateInfo { ty: i32, next: *const c_void, action: Handle, path: Handle, pose: Pose });
record!(ActionSuggestedBinding { action: Handle, binding: Handle });
record!(SuggestedBindings { ty: i32, next: *const c_void, profile: Handle, count: u32, bindings: *const ActionSuggestedBinding });
record!(SessionActionSetsAttachInfo { ty: i32, next: *const c_void, count: u32, sets: *const Handle });
record!(InteractionProfileState { ty: i32, next: *mut c_void, profile: Handle });
record!(ActionStateGetInfo { ty: i32, next: *const c_void, action: Handle, path: Handle });
record!(ActionStateBoolean { ty: i32, next: *mut c_void, current: u32, changed: u32, last_change: Time, active: u32 });
record!(ActionStateFloat { ty: i32, next: *mut c_void, current: f32, changed: u32, last_change: Time, active: u32 });
record!(ActionStateVector2f { ty: i32, next: *mut c_void, current: [f32; 2], changed: u32, last_change: Time, active: u32 });
record!(ActionStatePose { ty: i32, next: *mut c_void, active: u32 });
record!(ActiveActionSet { set: Handle, path: Handle });
record!(ActionsSyncInfo { ty: i32, next: *const c_void, count: u32, sets: *const ActiveActionSet });
record!(BoundSourcesInfo { ty: i32, next: *const c_void, action: Handle });
record!(InputSourceLocalizedNameInfo { ty: i32, next: *const c_void, source: Handle, components: u64 });
record!(HapticActionInfo { ty: i32, next: *const c_void, action: Handle, path: Handle });
record!(HapticVibration { ty: i32, next: *const c_void, duration: Time, frequency: f32, amplitude: f32 });
record!(GraphicsBindingVulkan { ty: i32, next: *const c_void, instance: *mut c_void, physical_device: *mut c_void, device: *mut c_void, queue_family: u32, queue_index: u32 });
record!(GraphicsRequirementsVulkan { ty: i32, next: *mut c_void, min_version: u64, max_version: u64 });
record!(VulkanGraphicsDeviceGetInfo { ty: i32, next: *const c_void, system: Handle, instance: *mut c_void });
record!(VulkanInstanceCreateInfo { ty: i32, next: *const c_void, system: Handle, flags: u64, get_instance_proc_addr: *const c_void, create_info: *const c_void, allocator: *const c_void });
record!(VulkanDeviceCreateInfo { ty: i32, next: *const c_void, system: Handle, flags: u64, get_instance_proc_addr: *const c_void, physical_device: *mut c_void, create_info: *const c_void, allocator: *const c_void });
record!(LoaderInfo { ty: i32, version: u32, size: usize, min_interface: u32, max_interface: u32, min_api: u64, max_api: u64 });
record!(RuntimeRequest { ty: i32, version: u32, size: usize, interface: u32, api: u64, get_instance_proc_addr: Option<unsafe extern "system" fn(Handle, *const c_char, *mut Proc) -> Result> });
