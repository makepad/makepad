#![allow(non_snake_case)]
use crate::module_loader::ModuleLoader;
use std::os::raw::c_char;
use std::os::raw::c_void;
use std::fmt;
use std::mem;
use std::ffi::CStr;

pub const MAX_APPLICATION_NAME_SIZE: usize = 128usize;
pub const MAX_ENGINE_NAME_SIZE: usize = 128usize;

#[repr(transparent)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct XrInstance(u64);

#[repr(transparent)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct XrVersion(u64);

#[repr(transparent)]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct XrInstanceCreateFlags(u64);

pub type XrVoidFunction = unsafe extern "system" fn();
pub type TxrGetInstanceProcAddr = unsafe extern "system" fn(
    instance: XrInstance,
    name: *const c_char,
    function: *mut Option<XrVoidFunction>,
) -> XrResult;

pub type TxrCreateInstance = unsafe extern "system" fn(
    create_info: *const XrInstanceCreateInfo,
    instance: *mut XrInstance,
) -> XrResult;

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct XrApplicationInfo {
    pub application_name: [c_char; MAX_APPLICATION_NAME_SIZE],
    pub application_version: u32,
    pub engine_name: [c_char; MAX_ENGINE_NAME_SIZE],
    pub engine_version: u32,
    pub api_version: XrVersion,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct XrInstanceCreateInfo {
    pub ty: XrStructureType,
    pub next: *const c_void,
    pub create_flags: XrInstanceCreateFlags,
    pub application_info: XrApplicationInfo,
    pub enabled_api_layer_count: u32,
    pub enabled_api_layer_names: *const *const c_char,
    pub enabled_extension_count: u32,
    pub enabled_extension_names: *const *const c_char,
}

pub struct LibOpenXr {
    xrCreateInstance:Option<TxrCreateInstance>,
    xrGetInstanceProcAddr: Option<TxrGetInstanceProcAddr>,
    /*
    xrAcquireSwapchainImage
    xrApplyHapticFeedback
    xrAttachSessionActionSets
    xrBeginFrame
    xrBeginSession
    xrCreateAction
    xrCreateActionSet
    xrCreateActionSpace
    xrCreateReferenceSpace
    xrCreateSession
    xrCreateSwapchain
    xrDestroyAction
    xrDestroyActionSet
    xrDestroyInstance
    xrDestroySession
    xrDestroySpace
    xrDestroySwapchain
    xrEndFrame
    xrEndSession
    xrEnumerateApiLayerProperties
    xrEnumerateBoundSourcesForAction
    xrEnumerateEnvironmentBlendModes
    xrEnumerateInstanceExtensionProperties
    xrEnumerateReferenceSpaces
    xrEnumerateSwapchainFormats
    xrEnumerateSwapchainImages
    xrEnumerateViewConfigurations
    xrEnumerateViewConfigurationViews
    xrGetActionStateBoolean
    xrGetActionStateFloat
    xrGetActionStatePose
    xrGetActionStateVector2f
    xrGetCurrentInteractionProfile
    xrGetInputSourceLocalizedName
    xrGetInstanceProcAddr
    xrGetInstanceProperties
    xrGetReferenceSpaceBoundsRect
    xrGetSystem
    xrGetSystemProperties
    xrGetViewConfigurationProperties
    xrLocateSpace
    xrLocateViews
    xrPathToString
    xrPollEvent
    xrReleaseSwapchainImage
    xrRequestExitSession
    xrResultToString
    xrStopHapticFeedback
    xrStringToPath
    xrStructureTypeToString
    xrSuggestInteractionProfileBindings
    xrSyncActions
    xrWaitFrame
    xrWaitSwapchainImage
    */
    _keep_module_alive: ModuleLoader,
}

impl LibOpenXr {
    pub fn try_load() -> Option<LibOpenXr> {
        let module = ModuleLoader::load("libopenxr_loader.so").ok()?;

        let gipa: Option<TxrGetInstanceProcAddr> = 
            module.get_symbol("xrGetInstanceProcAddr").ok();
            
        macro_rules! get_proc_addr {
            ($get_addr:expr, $inst:expr, $name:literal) => {
                unsafe{mem::transmute($get_addr.and_then(|addr| {
                    let mut f = None;
                    addr(
                        $inst, 
                        CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes()).unwrap().as_ptr(),
                        &mut f
                    );
                    f
                }))}
            };
        }
        let xrCreateInstance:Option<TxrCreateInstance> = get_proc_addr!(gipa, XrInstance(0), "xrCreateInstance");
        
        Some(LibOpenXr {
            xrGetInstanceProcAddr: gipa,
            xrCreateInstance,
            _keep_module_alive: module
        })
    }
}

#[repr(transparent)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct XrStructureType(i32);
impl XrStructureType {
    pub const UNKNOWN: XrStructureType = Self(0i32);
    pub const API_LAYER_PROPERTIES: XrStructureType = Self(1i32);
    pub const EXTENSION_PROPERTIES: XrStructureType = Self(2i32);
    pub const INSTANCE_CREATE_INFO: XrStructureType = Self(3i32);
    pub const SYSTEM_GET_INFO: XrStructureType = Self(4i32);
    pub const SYSTEM_PROPERTIES: XrStructureType = Self(5i32);
    pub const VIEW_LOCATE_INFO: XrStructureType = Self(6i32);
    pub const VIEW: XrStructureType = Self(7i32);
    pub const SESSION_CREATE_INFO: XrStructureType = Self(8i32);
    pub const SWAPCHAIN_CREATE_INFO: XrStructureType = Self(9i32);
    pub const SESSION_BEGIN_INFO: XrStructureType = Self(10i32);
    pub const VIEW_STATE: XrStructureType = Self(11i32);
    pub const FRAME_END_INFO: XrStructureType = Self(12i32);
    pub const HAPTIC_VIBRATION: XrStructureType = Self(13i32);
    pub const EVENT_DATA_BUFFER: XrStructureType = Self(16i32);
    pub const EVENT_DATA_INSTANCE_LOSS_PENDING: XrStructureType = Self(17i32);
    pub const EVENT_DATA_SESSION_STATE_CHANGED: XrStructureType = Self(18i32);
    pub const ACTION_STATE_BOOLEAN: XrStructureType = Self(23i32);
    pub const ACTION_STATE_FLOAT: XrStructureType = Self(24i32);
    pub const ACTION_STATE_VECTOR2F: XrStructureType = Self(25i32);
    pub const ACTION_STATE_POSE: XrStructureType = Self(27i32);
    pub const ACTION_SET_CREATE_INFO: XrStructureType = Self(28i32);
    pub const ACTION_CREATE_INFO: XrStructureType = Self(29i32);
    pub const INSTANCE_PROPERTIES: XrStructureType = Self(32i32);
    pub const FRAME_WAIT_INFO: XrStructureType = Self(33i32);
    pub const COMPOSITION_LAYER_PROJECTION: XrStructureType = Self(35i32);
    pub const COMPOSITION_LAYER_QUAD: XrStructureType = Self(36i32);
    pub const REFERENCE_SPACE_CREATE_INFO: XrStructureType = Self(37i32);
    pub const ACTION_SPACE_CREATE_INFO: XrStructureType = Self(38i32);
    pub const EVENT_DATA_REFERENCE_SPACE_CHANGE_PENDING: XrStructureType = Self(40i32);
    pub const VIEW_CONFIGURATION_VIEW: XrStructureType = Self(41i32);
    pub const SPACE_LOCATION: XrStructureType = Self(42i32);
    pub const SPACE_VELOCITY: XrStructureType = Self(43i32);
    pub const FRAME_STATE: XrStructureType = Self(44i32);
    pub const VIEW_CONFIGURATION_PROPERTIES: XrStructureType = Self(45i32);
    pub const FRAME_BEGIN_INFO: XrStructureType = Self(46i32);
    pub const COMPOSITION_LAYER_PROJECTION_VIEW: XrStructureType = Self(48i32);
    pub const EVENT_DATA_EVENTS_LOST: XrStructureType = Self(49i32);
    pub const INTERACTION_PROFILE_SUGGESTED_BINDING: XrStructureType = Self(51i32);
    pub const EVENT_DATA_INTERACTION_PROFILE_CHANGED: XrStructureType = Self(52i32);
    pub const INTERACTION_PROFILE_STATE: XrStructureType = Self(53i32);
    pub const SWAPCHAIN_IMAGE_ACQUIRE_INFO: XrStructureType = Self(55i32);
    pub const SWAPCHAIN_IMAGE_WAIT_INFO: XrStructureType = Self(56i32);
    pub const SWAPCHAIN_IMAGE_RELEASE_INFO: XrStructureType = Self(57i32);
    pub const ACTION_STATE_GET_INFO: XrStructureType = Self(58i32);
    pub const HAPTIC_ACTION_INFO: XrStructureType = Self(59i32);
    pub const SESSION_ACTION_SETS_ATTACH_INFO: XrStructureType = Self(60i32);
    pub const ACTIONS_SYNC_INFO: XrStructureType = Self(61i32);
    pub const BOUND_SOURCES_FOR_ACTION_ENUMERATE_INFO: XrStructureType = Self(62i32);
    pub const INPUT_SOURCE_LOCALIZED_NAME_GET_INFO: XrStructureType = Self(63i32);
    pub const SPACES_LOCATE_INFO: XrStructureType = Self(1000471000i32);
    pub const SPACE_LOCATIONS: XrStructureType = Self(1000471001i32);
    pub const SPACE_VELOCITIES: XrStructureType = Self(1000471002i32);
    pub const COMPOSITION_LAYER_CUBE_KHR: XrStructureType = Self(1000006000i32);
    pub const INSTANCE_CREATE_INFO_ANDROID_KHR: XrStructureType = Self(1000008000i32);
    pub const COMPOSITION_LAYER_DEPTH_INFO_KHR: XrStructureType = Self(1000010000i32);
    pub const VULKAN_SWAPCHAIN_FORMAT_LIST_CREATE_INFO_KHR: XrStructureType = Self(1000014000i32);
    pub const EVENT_DATA_PERF_SETTINGS_EXT: XrStructureType = Self(1000015000i32);
    pub const COMPOSITION_LAYER_CYLINDER_KHR: XrStructureType = Self(1000017000i32);
    pub const COMPOSITION_LAYER_EQUIRECT_KHR: XrStructureType = Self(1000018000i32);
    pub const DEBUG_UTILS_OBJECT_NAME_INFO_EXT: XrStructureType = Self(1000019000i32);
    pub const DEBUG_UTILS_MESSENGER_CALLBACK_DATA_EXT: XrStructureType = Self(1000019001i32);
    pub const DEBUG_UTILS_MESSENGER_CREATE_INFO_EXT: XrStructureType = Self(1000019002i32);
    pub const DEBUG_UTILS_LABEL_EXT: XrStructureType = Self(1000019003i32);
    pub const GRAPHICS_BINDING_OPENGL_WIN32_KHR: XrStructureType = Self(1000023000i32);
    pub const GRAPHICS_BINDING_OPENGL_XLIB_KHR: XrStructureType = Self(1000023001i32);
    pub const GRAPHICS_BINDING_OPENGL_XCB_KHR: XrStructureType = Self(1000023002i32);
    pub const GRAPHICS_BINDING_OPENGL_WAYLAND_KHR: XrStructureType = Self(1000023003i32);
    pub const SWAPCHAIN_IMAGE_OPENGL_KHR: XrStructureType = Self(1000023004i32);
    pub const GRAPHICS_REQUIREMENTS_OPENGL_KHR: XrStructureType = Self(1000023005i32);
    pub const GRAPHICS_BINDING_OPENGL_ES_ANDROID_KHR: XrStructureType = Self(1000024001i32);
    pub const SWAPCHAIN_IMAGE_OPENGL_ES_KHR: XrStructureType = Self(1000024002i32);
    pub const GRAPHICS_REQUIREMENTS_OPENGL_ES_KHR: XrStructureType = Self(1000024003i32);
    pub const GRAPHICS_BINDING_VULKAN_KHR: XrStructureType = Self(1000025000i32);
    pub const SWAPCHAIN_IMAGE_VULKAN_KHR: XrStructureType = Self(1000025001i32);
    pub const GRAPHICS_REQUIREMENTS_VULKAN_KHR: XrStructureType = Self(1000025002i32);
    pub const GRAPHICS_BINDING_D3D11_KHR: XrStructureType = Self(1000027000i32);
    pub const SWAPCHAIN_IMAGE_D3D11_KHR: XrStructureType = Self(1000027001i32);
    pub const GRAPHICS_REQUIREMENTS_D3D11_KHR: XrStructureType = Self(1000027002i32);
    pub const GRAPHICS_BINDING_D3D12_KHR: XrStructureType = Self(1000028000i32);
    pub const SWAPCHAIN_IMAGE_D3D12_KHR: XrStructureType = Self(1000028001i32);
    pub const GRAPHICS_REQUIREMENTS_D3D12_KHR: XrStructureType = Self(1000028002i32);
    pub const GRAPHICS_BINDING_METAL_KHR: XrStructureType = Self(1000029000i32);
    pub const SWAPCHAIN_IMAGE_METAL_KHR: XrStructureType = Self(1000029001i32);
    pub const GRAPHICS_REQUIREMENTS_METAL_KHR: XrStructureType = Self(1000029002i32);
    pub const SYSTEM_EYE_GAZE_INTERACTION_PROPERTIES_EXT: XrStructureType = Self(1000030000i32);
    pub const EYE_GAZE_SAMPLE_TIME_EXT: XrStructureType = Self(1000030001i32);
    pub const VISIBILITY_MASK_KHR: XrStructureType = Self(1000031000i32);
    pub const EVENT_DATA_VISIBILITY_MASK_CHANGED_KHR: XrStructureType = Self(1000031001i32);
    pub const SESSION_CREATE_INFO_OVERLAY_EXTX: XrStructureType = Self(1000033000i32);
    pub const EVENT_DATA_MAIN_SESSION_VISIBILITY_CHANGED_EXTX: XrStructureType = Self(1000033003i32);
    pub const COMPOSITION_LAYER_COLOR_SCALE_BIAS_KHR: XrStructureType = Self(1000034000i32);
    pub const SPATIAL_ANCHOR_CREATE_INFO_MSFT: XrStructureType = Self(1000039000i32);
    pub const SPATIAL_ANCHOR_SPACE_CREATE_INFO_MSFT: XrStructureType = Self(1000039001i32);
    pub const COMPOSITION_LAYER_IMAGE_LAYOUT_FB: XrStructureType = Self(1000040000i32);
    pub const COMPOSITION_LAYER_ALPHA_BLEND_FB: XrStructureType = Self(1000041001i32);
    pub const VIEW_CONFIGURATION_DEPTH_RANGE_EXT: XrStructureType = Self(1000046000i32);
    pub const GRAPHICS_BINDING_EGL_MNDX: XrStructureType = Self(1000048004i32);
    pub const SPATIAL_GRAPH_NODE_SPACE_CREATE_INFO_MSFT: XrStructureType = Self(1000049000i32);
    pub const SPATIAL_GRAPH_STATIC_NODE_BINDING_CREATE_INFO_MSFT: XrStructureType =
    Self(1000049001i32);
    pub const SPATIAL_GRAPH_NODE_BINDING_PROPERTIES_GET_INFO_MSFT: XrStructureType =
    Self(1000049002i32);
    pub const SPATIAL_GRAPH_NODE_BINDING_PROPERTIES_MSFT: XrStructureType = Self(1000049003i32);
    pub const SYSTEM_HAND_TRACKING_PROPERTIES_EXT: XrStructureType = Self(1000051000i32);
    pub const HAND_TRACKER_CREATE_INFO_EXT: XrStructureType = Self(1000051001i32);
    pub const HAND_JOINTS_LOCATE_INFO_EXT: XrStructureType = Self(1000051002i32);
    pub const HAND_JOINT_LOCATIONS_EXT: XrStructureType = Self(1000051003i32);
    pub const HAND_JOINT_VELOCITIES_EXT: XrStructureType = Self(1000051004i32);
    pub const SYSTEM_HAND_TRACKING_MESH_PROPERTIES_MSFT: XrStructureType = Self(1000052000i32);
    pub const HAND_MESH_SPACE_CREATE_INFO_MSFT: XrStructureType = Self(1000052001i32);
    pub const HAND_MESH_UPDATE_INFO_MSFT: XrStructureType = Self(1000052002i32);
    pub const HAND_MESH_MSFT: XrStructureType = Self(1000052003i32);
    pub const HAND_POSE_TYPE_INFO_MSFT: XrStructureType = Self(1000052004i32);
    pub const SECONDARY_VIEW_CONFIGURATION_SESSION_BEGIN_INFO_MSFT: XrStructureType =
    Self(1000053000i32);
    pub const SECONDARY_VIEW_CONFIGURATION_STATE_MSFT: XrStructureType = Self(1000053001i32);
    pub const SECONDARY_VIEW_CONFIGURATION_FRAME_STATE_MSFT: XrStructureType = Self(1000053002i32);
    pub const SECONDARY_VIEW_CONFIGURATION_FRAME_END_INFO_MSFT: XrStructureType = Self(1000053003i32);
    pub const SECONDARY_VIEW_CONFIGURATION_LAYER_INFO_MSFT: XrStructureType = Self(1000053004i32);
    pub const SECONDARY_VIEW_CONFIGURATION_SWAPCHAIN_CREATE_INFO_MSFT: XrStructureType =
    Self(1000053005i32);
    pub const CONTROLLER_MODEL_KEY_STATE_MSFT: XrStructureType = Self(1000055000i32);
    pub const CONTROLLER_MODEL_NODE_PROPERTIES_MSFT: XrStructureType = Self(1000055001i32);
    pub const CONTROLLER_MODEL_PROPERTIES_MSFT: XrStructureType = Self(1000055002i32);
    pub const CONTROLLER_MODEL_NODE_STATE_MSFT: XrStructureType = Self(1000055003i32);
    pub const CONTROLLER_MODEL_STATE_MSFT: XrStructureType = Self(1000055004i32);
    pub const VIEW_CONFIGURATION_VIEW_FOV_EPIC: XrStructureType = Self(1000059000i32);
    pub const HOLOGRAPHIC_WINDOW_ATTACHMENT_MSFT: XrStructureType = Self(1000063000i32);
    pub const COMPOSITION_LAYER_REPROJECTION_INFO_MSFT: XrStructureType = Self(1000066000i32);
    pub const COMPOSITION_LAYER_REPROJECTION_PLANE_OVERRIDE_MSFT: XrStructureType =
    Self(1000066001i32);
    pub const ANDROID_SURFACE_SWAPCHAIN_CREATE_INFO_FB: XrStructureType = Self(1000070000i32);
    pub const COMPOSITION_LAYER_SECURE_CONTENT_FB: XrStructureType = Self(1000072000i32);
    pub const BODY_TRACKER_CREATE_INFO_FB: XrStructureType = Self(1000076001i32);
    pub const BODY_JOINTS_LOCATE_INFO_FB: XrStructureType = Self(1000076002i32);
    pub const SYSTEM_BODY_TRACKING_PROPERTIES_FB: XrStructureType = Self(1000076004i32);
    pub const BODY_JOINT_LOCATIONS_FB: XrStructureType = Self(1000076005i32);
    pub const BODY_SKELETON_FB: XrStructureType = Self(1000076006i32);
    pub const INTERACTION_PROFILE_DPAD_BINDING_EXT: XrStructureType = Self(1000078000i32);
    pub const INTERACTION_PROFILE_ANALOG_THRESHOLD_VALVE: XrStructureType = Self(1000079000i32);
    pub const HAND_JOINTS_MOTION_RANGE_INFO_EXT: XrStructureType = Self(1000080000i32);
    pub const LOADER_INIT_INFO_ANDROID_KHR: XrStructureType = Self(1000089000i32);
    pub const VULKAN_INSTANCE_CREATE_INFO_KHR: XrStructureType = Self(1000090000i32);
    pub const VULKAN_DEVICE_CREATE_INFO_KHR: XrStructureType = Self(1000090001i32);
    pub const VULKAN_GRAPHICS_DEVICE_GET_INFO_KHR: XrStructureType = Self(1000090003i32);
    pub const GRAPHICS_BINDING_VULKAN2_KHR: XrStructureType = Self::GRAPHICS_BINDING_VULKAN_KHR;
    pub const SWAPCHAIN_IMAGE_VULKAN2_KHR: XrStructureType = Self::SWAPCHAIN_IMAGE_VULKAN_KHR;
    pub const GRAPHICS_REQUIREMENTS_VULKAN2_KHR: XrStructureType =
    Self::GRAPHICS_REQUIREMENTS_VULKAN_KHR;
    pub const COMPOSITION_LAYER_EQUIRECT2_KHR: XrStructureType = Self(1000091000i32);
    pub const SCENE_OBSERVER_CREATE_INFO_MSFT: XrStructureType = Self(1000097000i32);
    pub const SCENE_CREATE_INFO_MSFT: XrStructureType = Self(1000097001i32);
    pub const NEW_SCENE_COMPUTE_INFO_MSFT: XrStructureType = Self(1000097002i32);
    pub const VISUAL_MESH_COMPUTE_LOD_INFO_MSFT: XrStructureType = Self(1000097003i32);
    pub const SCENE_COMPONENTS_MSFT: XrStructureType = Self(1000097004i32);
    pub const SCENE_COMPONENTS_GET_INFO_MSFT: XrStructureType = Self(1000097005i32);
    pub const SCENE_COMPONENT_LOCATIONS_MSFT: XrStructureType = Self(1000097006i32);
    pub const SCENE_COMPONENTS_LOCATE_INFO_MSFT: XrStructureType = Self(1000097007i32);
    pub const SCENE_OBJECTS_MSFT: XrStructureType = Self(1000097008i32);
    pub const SCENE_COMPONENT_PARENT_FILTER_INFO_MSFT: XrStructureType = Self(1000097009i32);
    pub const SCENE_OBJECT_TYPES_FILTER_INFO_MSFT: XrStructureType = Self(1000097010i32);
    pub const SCENE_PLANES_MSFT: XrStructureType = Self(1000097011i32);
    pub const SCENE_PLANE_ALIGNMENT_FILTER_INFO_MSFT: XrStructureType = Self(1000097012i32);
    pub const SCENE_MESHES_MSFT: XrStructureType = Self(1000097013i32);
    pub const SCENE_MESH_BUFFERS_GET_INFO_MSFT: XrStructureType = Self(1000097014i32);
    pub const SCENE_MESH_BUFFERS_MSFT: XrStructureType = Self(1000097015i32);
    pub const SCENE_MESH_VERTEX_BUFFER_MSFT: XrStructureType = Self(1000097016i32);
    pub const SCENE_MESH_INDICES_UINT32_MSFT: XrStructureType = Self(1000097017i32);
    pub const SCENE_MESH_INDICES_UINT16_MSFT: XrStructureType = Self(1000097018i32);
    pub const SERIALIZED_SCENE_FRAGMENT_DATA_GET_INFO_MSFT: XrStructureType = Self(1000098000i32);
    pub const SCENE_DESERIALIZE_INFO_MSFT: XrStructureType = Self(1000098001i32);
    pub const EVENT_DATA_DISPLAY_REFRESH_RATE_CHANGED_FB: XrStructureType = Self(1000101000i32);
    pub const VIVE_TRACKER_PATHS_HTCX: XrStructureType = Self(1000103000i32);
    pub const EVENT_DATA_VIVE_TRACKER_CONNECTED_HTCX: XrStructureType = Self(1000103001i32);
    pub const SYSTEM_FACIAL_TRACKING_PROPERTIES_HTC: XrStructureType = Self(1000104000i32);
    pub const FACIAL_TRACKER_CREATE_INFO_HTC: XrStructureType = Self(1000104001i32);
    pub const FACIAL_EXPRESSIONS_HTC: XrStructureType = Self(1000104002i32);
    pub const SYSTEM_COLOR_SPACE_PROPERTIES_FB: XrStructureType = Self(1000108000i32);
    pub const HAND_TRACKING_MESH_FB: XrStructureType = Self(1000110001i32);
    pub const HAND_TRACKING_SCALE_FB: XrStructureType = Self(1000110003i32);
    pub const HAND_TRACKING_AIM_STATE_FB: XrStructureType = Self(1000111001i32);
    pub const HAND_TRACKING_CAPSULES_STATE_FB: XrStructureType = Self(1000112000i32);
    pub const SYSTEM_SPATIAL_ENTITY_PROPERTIES_FB: XrStructureType = Self(1000113004i32);
    pub const SPATIAL_ANCHOR_CREATE_INFO_FB: XrStructureType = Self(1000113003i32);
    pub const SPACE_COMPONENT_STATUS_SET_INFO_FB: XrStructureType = Self(1000113007i32);
    pub const SPACE_COMPONENT_STATUS_FB: XrStructureType = Self(1000113001i32);
    pub const EVENT_DATA_SPATIAL_ANCHOR_CREATE_COMPLETE_FB: XrStructureType = Self(1000113005i32);
    pub const EVENT_DATA_SPACE_SET_STATUS_COMPLETE_FB: XrStructureType = Self(1000113006i32);
    pub const FOVEATION_PROFILE_CREATE_INFO_FB: XrStructureType = Self(1000114000i32);
    pub const SWAPCHAIN_CREATE_INFO_FOVEATION_FB: XrStructureType = Self(1000114001i32);
    pub const SWAPCHAIN_STATE_FOVEATION_FB: XrStructureType = Self(1000114002i32);
    pub const FOVEATION_LEVEL_PROFILE_CREATE_INFO_FB: XrStructureType = Self(1000115000i32);
    pub const KEYBOARD_SPACE_CREATE_INFO_FB: XrStructureType = Self(1000116009i32);
    pub const KEYBOARD_TRACKING_QUERY_FB: XrStructureType = Self(1000116004i32);
    pub const SYSTEM_KEYBOARD_TRACKING_PROPERTIES_FB: XrStructureType = Self(1000116002i32);
    pub const TRIANGLE_MESH_CREATE_INFO_FB: XrStructureType = Self(1000117001i32);
    pub const SYSTEM_PASSTHROUGH_PROPERTIES_FB: XrStructureType = Self(1000118000i32);
    pub const PASSTHROUGH_CREATE_INFO_FB: XrStructureType = Self(1000118001i32);
    pub const PASSTHROUGH_LAYER_CREATE_INFO_FB: XrStructureType = Self(1000118002i32);
    pub const COMPOSITION_LAYER_PASSTHROUGH_FB: XrStructureType = Self(1000118003i32);
    pub const GEOMETRY_INSTANCE_CREATE_INFO_FB: XrStructureType = Self(1000118004i32);
    pub const GEOMETRY_INSTANCE_TRANSFORM_FB: XrStructureType = Self(1000118005i32);
    pub const SYSTEM_PASSTHROUGH_PROPERTIES2_FB: XrStructureType = Self(1000118006i32);
    pub const PASSTHROUGH_STYLE_FB: XrStructureType = Self(1000118020i32);
    pub const PASSTHROUGH_COLOR_MAP_MONO_TO_RGBA_FB: XrStructureType = Self(1000118021i32);
    pub const PASSTHROUGH_COLOR_MAP_MONO_TO_MONO_FB: XrStructureType = Self(1000118022i32);
    pub const PASSTHROUGH_BRIGHTNESS_CONTRAST_SATURATION_FB: XrStructureType = Self(1000118023i32);
    pub const EVENT_DATA_PASSTHROUGH_STATE_CHANGED_FB: XrStructureType = Self(1000118030i32);
    pub const RENDER_MODEL_PATH_INFO_FB: XrStructureType = Self(1000119000i32);
    pub const RENDER_MODEL_PROPERTIES_FB: XrStructureType = Self(1000119001i32);
    pub const RENDER_MODEL_BUFFER_FB: XrStructureType = Self(1000119002i32);
    pub const RENDER_MODEL_LOAD_INFO_FB: XrStructureType = Self(1000119003i32);
    pub const SYSTEM_RENDER_MODEL_PROPERTIES_FB: XrStructureType = Self(1000119004i32);
    pub const RENDER_MODEL_CAPABILITIES_REQUEST_FB: XrStructureType = Self(1000119005i32);
    pub const BINDING_MODIFICATIONS_KHR: XrStructureType = Self(1000120000i32);
    pub const VIEW_LOCATE_FOVEATED_RENDERING_VARJO: XrStructureType = Self(1000121000i32);
    pub const FOVEATED_VIEW_CONFIGURATION_VIEW_VARJO: XrStructureType = Self(1000121001i32);
    pub const SYSTEM_FOVEATED_RENDERING_PROPERTIES_VARJO: XrStructureType = Self(1000121002i32);
    pub const COMPOSITION_LAYER_DEPTH_TEST_VARJO: XrStructureType = Self(1000122000i32);
    pub const SYSTEM_MARKER_TRACKING_PROPERTIES_VARJO: XrStructureType = Self(1000124000i32);
    pub const EVENT_DATA_MARKER_TRACKING_UPDATE_VARJO: XrStructureType = Self(1000124001i32);
    pub const MARKER_SPACE_CREATE_INFO_VARJO: XrStructureType = Self(1000124002i32);
    pub const FRAME_END_INFO_ML: XrStructureType = Self(1000135000i32);
    pub const GLOBAL_DIMMER_FRAME_END_INFO_ML: XrStructureType = Self(1000136000i32);
    pub const COORDINATE_SPACE_CREATE_INFO_ML: XrStructureType = Self(1000137000i32);
    pub const SYSTEM_MARKER_UNDERSTANDING_PROPERTIES_ML: XrStructureType = Self(1000138000i32);
    pub const MARKER_DETECTOR_CREATE_INFO_ML: XrStructureType = Self(1000138001i32);
    pub const MARKER_DETECTOR_ARUCO_INFO_ML: XrStructureType = Self(1000138002i32);
    pub const MARKER_DETECTOR_SIZE_INFO_ML: XrStructureType = Self(1000138003i32);
    pub const MARKER_DETECTOR_APRIL_TAG_INFO_ML: XrStructureType = Self(1000138004i32);
    pub const MARKER_DETECTOR_CUSTOM_PROFILE_INFO_ML: XrStructureType = Self(1000138005i32);
    pub const MARKER_DETECTOR_SNAPSHOT_INFO_ML: XrStructureType = Self(1000138006i32);
    pub const MARKER_DETECTOR_STATE_ML: XrStructureType = Self(1000138007i32);
    pub const MARKER_SPACE_CREATE_INFO_ML: XrStructureType = Self(1000138008i32);
    pub const LOCALIZATION_MAP_ML: XrStructureType = Self(1000139000i32);
    pub const EVENT_DATA_LOCALIZATION_CHANGED_ML: XrStructureType = Self(1000139001i32);
    pub const MAP_LOCALIZATION_REQUEST_INFO_ML: XrStructureType = Self(1000139002i32);
    pub const LOCALIZATION_MAP_IMPORT_INFO_ML: XrStructureType = Self(1000139003i32);
    pub const LOCALIZATION_ENABLE_EVENTS_INFO_ML: XrStructureType = Self(1000139004i32);
    pub const EVENT_DATA_HEADSET_FIT_CHANGED_ML: XrStructureType = Self(1000472000i32);
    pub const EVENT_DATA_EYE_CALIBRATION_CHANGED_ML: XrStructureType = Self(1000472001i32);
    pub const USER_CALIBRATION_ENABLE_EVENTS_INFO_ML: XrStructureType = Self(1000472002i32);
    pub const SPATIAL_ANCHOR_PERSISTENCE_INFO_MSFT: XrStructureType = Self(1000142000i32);
    pub const SPATIAL_ANCHOR_FROM_PERSISTED_ANCHOR_CREATE_INFO_MSFT: XrStructureType =
    Self(1000142001i32);
    pub const SCENE_MARKERS_MSFT: XrStructureType = Self(1000147000i32);
    pub const SCENE_MARKER_TYPE_FILTER_MSFT: XrStructureType = Self(1000147001i32);
    pub const SCENE_MARKER_QR_CODES_MSFT: XrStructureType = Self(1000147002i32);
    pub const SPACE_QUERY_INFO_FB: XrStructureType = Self(1000156001i32);
    pub const SPACE_QUERY_RESULTS_FB: XrStructureType = Self(1000156002i32);
    pub const SPACE_STORAGE_LOCATION_FILTER_INFO_FB: XrStructureType = Self(1000156003i32);
    pub const SPACE_UUID_FILTER_INFO_FB: XrStructureType = Self(1000156054i32);
    pub const SPACE_COMPONENT_FILTER_INFO_FB: XrStructureType = Self(1000156052i32);
    pub const EVENT_DATA_SPACE_QUERY_RESULTS_AVAILABLE_FB: XrStructureType = Self(1000156103i32);
    pub const EVENT_DATA_SPACE_QUERY_COMPLETE_FB: XrStructureType = Self(1000156104i32);
    pub const SPACE_SAVE_INFO_FB: XrStructureType = Self(1000158000i32);
    pub const SPACE_ERASE_INFO_FB: XrStructureType = Self(1000158001i32);
    pub const EVENT_DATA_SPACE_SAVE_COMPLETE_FB: XrStructureType = Self(1000158106i32);
    pub const EVENT_DATA_SPACE_ERASE_COMPLETE_FB: XrStructureType = Self(1000158107i32);
    pub const SWAPCHAIN_IMAGE_FOVEATION_VULKAN_FB: XrStructureType = Self(1000160000i32);
    pub const SWAPCHAIN_STATE_ANDROID_SURFACE_DIMENSIONS_FB: XrStructureType = Self(1000161000i32);
    pub const SWAPCHAIN_STATE_SAMPLER_OPENGL_ES_FB: XrStructureType = Self(1000162000i32);
    pub const SWAPCHAIN_STATE_SAMPLER_VULKAN_FB: XrStructureType = Self(1000163000i32);
    pub const SPACE_SHARE_INFO_FB: XrStructureType = Self(1000169001i32);
    pub const EVENT_DATA_SPACE_SHARE_COMPLETE_FB: XrStructureType = Self(1000169002i32);
    pub const COMPOSITION_LAYER_SPACE_WARP_INFO_FB: XrStructureType = Self(1000171000i32);
    pub const SYSTEM_SPACE_WARP_PROPERTIES_FB: XrStructureType = Self(1000171001i32);
    pub const HAPTIC_AMPLITUDE_ENVELOPE_VIBRATION_FB: XrStructureType = Self(1000173001i32);
    pub const SEMANTIC_LABELS_FB: XrStructureType = Self(1000175000i32);
    pub const ROOM_LAYOUT_FB: XrStructureType = Self(1000175001i32);
    pub const BOUNDARY_2D_FB: XrStructureType = Self(1000175002i32);
    pub const SEMANTIC_LABELS_SUPPORT_INFO_FB: XrStructureType = Self(1000175010i32);
    pub const DIGITAL_LENS_CONTROL_ALMALENCE: XrStructureType = Self(1000196000i32);
    pub const EVENT_DATA_SCENE_CAPTURE_COMPLETE_FB: XrStructureType = Self(1000198001i32);
    pub const SCENE_CAPTURE_REQUEST_INFO_FB: XrStructureType = Self(1000198050i32);
    pub const SPACE_CONTAINER_FB: XrStructureType = Self(1000199000i32);
    pub const FOVEATION_EYE_TRACKED_PROFILE_CREATE_INFO_META: XrStructureType = Self(1000200000i32);
    pub const FOVEATION_EYE_TRACKED_STATE_META: XrStructureType = Self(1000200001i32);
    pub const SYSTEM_FOVEATION_EYE_TRACKED_PROPERTIES_META: XrStructureType = Self(1000200002i32);
    pub const SYSTEM_FACE_TRACKING_PROPERTIES_FB: XrStructureType = Self(1000201004i32);
    pub const FACE_TRACKER_CREATE_INFO_FB: XrStructureType = Self(1000201005i32);
    pub const FACE_EXPRESSION_INFO_FB: XrStructureType = Self(1000201002i32);
    pub const FACE_EXPRESSION_WEIGHTS_FB: XrStructureType = Self(1000201006i32);
    pub const EYE_TRACKER_CREATE_INFO_FB: XrStructureType = Self(1000202001i32);
    pub const EYE_GAZES_INFO_FB: XrStructureType = Self(1000202002i32);
    pub const EYE_GAZES_FB: XrStructureType = Self(1000202003i32);
    pub const SYSTEM_EYE_TRACKING_PROPERTIES_FB: XrStructureType = Self(1000202004i32);
    pub const PASSTHROUGH_KEYBOARD_HANDS_INTENSITY_FB: XrStructureType = Self(1000203002i32);
    pub const COMPOSITION_LAYER_SETTINGS_FB: XrStructureType = Self(1000204000i32);
    pub const HAPTIC_PCM_VIBRATION_FB: XrStructureType = Self(1000209001i32);
    pub const DEVICE_PCM_SAMPLE_RATE_STATE_FB: XrStructureType = Self(1000209002i32);
    pub const DEVICE_PCM_SAMPLE_RATE_GET_INFO_FB: XrStructureType =
    Self::DEVICE_PCM_SAMPLE_RATE_STATE_FB;
    pub const COMPOSITION_LAYER_DEPTH_TEST_FB: XrStructureType = Self(1000212000i32);
    pub const LOCAL_DIMMING_FRAME_END_INFO_META: XrStructureType = Self(1000216000i32);
    pub const PASSTHROUGH_PREFERENCES_META: XrStructureType = Self(1000217000i32);
    pub const SYSTEM_VIRTUAL_KEYBOARD_PROPERTIES_META: XrStructureType = Self(1000219001i32);
    pub const VIRTUAL_KEYBOARD_CREATE_INFO_META: XrStructureType = Self(1000219002i32);
    pub const VIRTUAL_KEYBOARD_SPACE_CREATE_INFO_META: XrStructureType = Self(1000219003i32);
    pub const VIRTUAL_KEYBOARD_LOCATION_INFO_META: XrStructureType = Self(1000219004i32);
    pub const VIRTUAL_KEYBOARD_MODEL_VISIBILITY_SET_INFO_META: XrStructureType = Self(1000219005i32);
    pub const VIRTUAL_KEYBOARD_ANIMATION_STATE_META: XrStructureType = Self(1000219006i32);
    pub const VIRTUAL_KEYBOARD_MODEL_ANIMATION_STATES_META: XrStructureType = Self(1000219007i32);
    pub const VIRTUAL_KEYBOARD_TEXTURE_DATA_META: XrStructureType = Self(1000219009i32);
    pub const VIRTUAL_KEYBOARD_INPUT_INFO_META: XrStructureType = Self(1000219010i32);
    pub const VIRTUAL_KEYBOARD_TEXT_CONTEXT_CHANGE_INFO_META: XrStructureType = Self(1000219011i32);
    pub const EVENT_DATA_VIRTUAL_KEYBOARD_COMMIT_TEXT_META: XrStructureType = Self(1000219014i32);
    pub const EVENT_DATA_VIRTUAL_KEYBOARD_BACKSPACE_META: XrStructureType = Self(1000219015i32);
    pub const EVENT_DATA_VIRTUAL_KEYBOARD_ENTER_META: XrStructureType = Self(1000219016i32);
    pub const EVENT_DATA_VIRTUAL_KEYBOARD_SHOWN_META: XrStructureType = Self(1000219017i32);
    pub const EVENT_DATA_VIRTUAL_KEYBOARD_HIDDEN_META: XrStructureType = Self(1000219018i32);
    pub const EXTERNAL_CAMERA_OCULUS: XrStructureType = Self(1000226000i32);
    pub const VULKAN_SWAPCHAIN_CREATE_INFO_META: XrStructureType = Self(1000227000i32);
    pub const PERFORMANCE_METRICS_STATE_META: XrStructureType = Self(1000232001i32);
    pub const PERFORMANCE_METRICS_COUNTER_META: XrStructureType = Self(1000232002i32);
    pub const SPACE_LIST_SAVE_INFO_FB: XrStructureType = Self(1000238000i32);
    pub const EVENT_DATA_SPACE_LIST_SAVE_COMPLETE_FB: XrStructureType = Self(1000238001i32);
    pub const SPACE_USER_CREATE_INFO_FB: XrStructureType = Self(1000241001i32);
    pub const SYSTEM_HEADSET_ID_PROPERTIES_META: XrStructureType = Self(1000245000i32);
    pub const RECOMMENDED_LAYER_RESOLUTION_META: XrStructureType = Self(1000254000i32);
    pub const RECOMMENDED_LAYER_RESOLUTION_GET_INFO_META: XrStructureType = Self(1000254001i32);
    pub const SYSTEM_PASSTHROUGH_COLOR_LUT_PROPERTIES_META: XrStructureType = Self(1000266000i32);
    pub const PASSTHROUGH_COLOR_LUT_CREATE_INFO_META: XrStructureType = Self(1000266001i32);
    pub const PASSTHROUGH_COLOR_LUT_UPDATE_INFO_META: XrStructureType = Self(1000266002i32);
    pub const PASSTHROUGH_COLOR_MAP_LUT_META: XrStructureType = Self(1000266100i32);
    pub const PASSTHROUGH_COLOR_MAP_INTERPOLATED_LUT_META: XrStructureType = Self(1000266101i32);
    pub const SPACE_TRIANGLE_MESH_GET_INFO_META: XrStructureType = Self(1000269001i32);
    pub const SPACE_TRIANGLE_MESH_META: XrStructureType = Self(1000269002i32);
    pub const SYSTEM_FACE_TRACKING_PROPERTIES2_FB: XrStructureType = Self(1000287013i32);
    pub const FACE_TRACKER_CREATE_INFO2_FB: XrStructureType = Self(1000287014i32);
    pub const FACE_EXPRESSION_INFO2_FB: XrStructureType = Self(1000287015i32);
    pub const FACE_EXPRESSION_WEIGHTS2_FB: XrStructureType = Self(1000287016i32);
    pub const ENVIRONMENT_DEPTH_PROVIDER_CREATE_INFO_META: XrStructureType = Self(1000291000i32);
    pub const ENVIRONMENT_DEPTH_SWAPCHAIN_CREATE_INFO_META: XrStructureType = Self(1000291001i32);
    pub const ENVIRONMENT_DEPTH_SWAPCHAIN_STATE_META: XrStructureType = Self(1000291002i32);
    pub const ENVIRONMENT_DEPTH_IMAGE_ACQUIRE_INFO_META: XrStructureType = Self(1000291003i32);
    pub const ENVIRONMENT_DEPTH_IMAGE_VIEW_META: XrStructureType = Self(1000291004i32);
    pub const ENVIRONMENT_DEPTH_IMAGE_META: XrStructureType = Self(1000291005i32);
    pub const ENVIRONMENT_DEPTH_HAND_REMOVAL_SET_INFO_META: XrStructureType = Self(1000291006i32);
    pub const SYSTEM_ENVIRONMENT_DEPTH_PROPERTIES_META: XrStructureType = Self(1000291007i32);
    pub const PASSTHROUGH_CREATE_INFO_HTC: XrStructureType = Self(1000317001i32);
    pub const PASSTHROUGH_COLOR_HTC: XrStructureType = Self(1000317002i32);
    pub const PASSTHROUGH_MESH_TRANSFORM_INFO_HTC: XrStructureType = Self(1000317003i32);
    pub const COMPOSITION_LAYER_PASSTHROUGH_HTC: XrStructureType = Self(1000317004i32);
    pub const FOVEATION_APPLY_INFO_HTC: XrStructureType = Self(1000318000i32);
    pub const FOVEATION_DYNAMIC_MODE_INFO_HTC: XrStructureType = Self(1000318001i32);
    pub const FOVEATION_CUSTOM_MODE_INFO_HTC: XrStructureType = Self(1000318002i32);
    pub const SYSTEM_ANCHOR_PROPERTIES_HTC: XrStructureType = Self(1000319000i32);
    pub const SPATIAL_ANCHOR_CREATE_INFO_HTC: XrStructureType = Self(1000319001i32);
    pub const ACTIVE_ACTION_SET_PRIORITIES_EXT: XrStructureType = Self(1000373000i32);
    pub const SYSTEM_FORCE_FEEDBACK_CURL_PROPERTIES_MNDX: XrStructureType = Self(1000375000i32);
    pub const FORCE_FEEDBACK_CURL_APPLY_LOCATIONS_MNDX: XrStructureType = Self(1000375001i32);
    pub const HAND_TRACKING_DATA_SOURCE_INFO_EXT: XrStructureType = Self(1000428000i32);
    pub const HAND_TRACKING_DATA_SOURCE_STATE_EXT: XrStructureType = Self(1000428001i32);
    pub const PLANE_DETECTOR_CREATE_INFO_EXT: XrStructureType = Self(1000429001i32);
    pub const PLANE_DETECTOR_BEGIN_INFO_EXT: XrStructureType = Self(1000429002i32);
    pub const PLANE_DETECTOR_GET_INFO_EXT: XrStructureType = Self(1000429003i32);
    pub const PLANE_DETECTOR_LOCATIONS_EXT: XrStructureType = Self(1000429004i32);
    pub const PLANE_DETECTOR_LOCATION_EXT: XrStructureType = Self(1000429005i32);
    pub const PLANE_DETECTOR_POLYGON_BUFFER_EXT: XrStructureType = Self(1000429006i32);
    pub const SYSTEM_PLANE_DETECTION_PROPERTIES_EXT: XrStructureType = Self(1000429007i32);
    pub const FUTURE_CANCEL_INFO_EXT: XrStructureType = Self(1000469000i32);
    pub const FUTURE_POLL_INFO_EXT: XrStructureType = Self(1000469001i32);
    pub const FUTURE_COMPLETION_EXT: XrStructureType = Self(1000469002i32);
    pub const FUTURE_POLL_RESULT_EXT: XrStructureType = Self(1000469003i32);
    pub const EVENT_DATA_USER_PRESENCE_CHANGED_EXT: XrStructureType = Self(1000470000i32);
    pub const SYSTEM_USER_PRESENCE_PROPERTIES_EXT: XrStructureType = Self(1000470001i32);
    pub const SPACES_LOCATE_INFO_KHR: XrStructureType = Self::SPACES_LOCATE_INFO;
    pub const SPACE_LOCATIONS_KHR: XrStructureType = Self::SPACE_LOCATIONS;
    pub const SPACE_VELOCITIES_KHR: XrStructureType = Self::SPACE_VELOCITIES;
    pub fn from_raw(x: i32) -> Self {
        Self(x)
    }
    pub fn into_raw(self) -> i32 {
        self.0
    }
}

impl fmt::Debug for XrStructureType {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let name = match *self {
            Self::UNKNOWN => Some("UNKNOWN"),
            Self::API_LAYER_PROPERTIES => Some("API_LAYER_PROPERTIES"),
            Self::EXTENSION_PROPERTIES => Some("EXTENSION_PROPERTIES"),
            Self::INSTANCE_CREATE_INFO => Some("INSTANCE_CREATE_INFO"),
            Self::SYSTEM_GET_INFO => Some("SYSTEM_GET_INFO"),
            Self::SYSTEM_PROPERTIES => Some("SYSTEM_PROPERTIES"),
            Self::VIEW_LOCATE_INFO => Some("VIEW_LOCATE_INFO"),
            Self::VIEW => Some("VIEW"),
            Self::SESSION_CREATE_INFO => Some("SESSION_CREATE_INFO"),
            Self::SWAPCHAIN_CREATE_INFO => Some("SWAPCHAIN_CREATE_INFO"),
            Self::SESSION_BEGIN_INFO => Some("SESSION_BEGIN_INFO"),
            Self::VIEW_STATE => Some("VIEW_STATE"),
            Self::FRAME_END_INFO => Some("FRAME_END_INFO"),
            Self::HAPTIC_VIBRATION => Some("HAPTIC_VIBRATION"),
            Self::EVENT_DATA_BUFFER => Some("EVENT_DATA_BUFFER"),
            Self::EVENT_DATA_INSTANCE_LOSS_PENDING => Some("EVENT_DATA_INSTANCE_LOSS_PENDING"),
            Self::EVENT_DATA_SESSION_STATE_CHANGED => Some("EVENT_DATA_SESSION_STATE_CHANGED"),
            Self::ACTION_STATE_BOOLEAN => Some("ACTION_STATE_BOOLEAN"),
            Self::ACTION_STATE_FLOAT => Some("ACTION_STATE_FLOAT"),
            Self::ACTION_STATE_VECTOR2F => Some("ACTION_STATE_VECTOR2F"),
            Self::ACTION_STATE_POSE => Some("ACTION_STATE_POSE"),
            Self::ACTION_SET_CREATE_INFO => Some("ACTION_SET_CREATE_INFO"),
            Self::ACTION_CREATE_INFO => Some("ACTION_CREATE_INFO"),
            Self::INSTANCE_PROPERTIES => Some("INSTANCE_PROPERTIES"),
            Self::FRAME_WAIT_INFO => Some("FRAME_WAIT_INFO"),
            Self::COMPOSITION_LAYER_PROJECTION => Some("COMPOSITION_LAYER_PROJECTION"),
            Self::COMPOSITION_LAYER_QUAD => Some("COMPOSITION_LAYER_QUAD"),
            Self::REFERENCE_SPACE_CREATE_INFO => Some("REFERENCE_SPACE_CREATE_INFO"),
            Self::ACTION_SPACE_CREATE_INFO => Some("ACTION_SPACE_CREATE_INFO"),
            Self::EVENT_DATA_REFERENCE_SPACE_CHANGE_PENDING => {
                Some("EVENT_DATA_REFERENCE_SPACE_CHANGE_PENDING")
            }
            Self::VIEW_CONFIGURATION_VIEW => Some("VIEW_CONFIGURATION_VIEW"),
            Self::SPACE_LOCATION => Some("SPACE_LOCATION"),
            Self::SPACE_VELOCITY => Some("SPACE_VELOCITY"),
            Self::FRAME_STATE => Some("FRAME_STATE"),
            Self::VIEW_CONFIGURATION_PROPERTIES => Some("VIEW_CONFIGURATION_PROPERTIES"),
            Self::FRAME_BEGIN_INFO => Some("FRAME_BEGIN_INFO"),
            Self::COMPOSITION_LAYER_PROJECTION_VIEW => Some("COMPOSITION_LAYER_PROJECTION_VIEW"),
            Self::EVENT_DATA_EVENTS_LOST => Some("EVENT_DATA_EVENTS_LOST"),
            Self::INTERACTION_PROFILE_SUGGESTED_BINDING => {
                Some("INTERACTION_PROFILE_SUGGESTED_BINDING")
            }
            Self::EVENT_DATA_INTERACTION_PROFILE_CHANGED => {
                Some("EVENT_DATA_INTERACTION_PROFILE_CHANGED")
            }
            Self::INTERACTION_PROFILE_STATE => Some("INTERACTION_PROFILE_STATE"),
            Self::SWAPCHAIN_IMAGE_ACQUIRE_INFO => Some("SWAPCHAIN_IMAGE_ACQUIRE_INFO"),
            Self::SWAPCHAIN_IMAGE_WAIT_INFO => Some("SWAPCHAIN_IMAGE_WAIT_INFO"),
            Self::SWAPCHAIN_IMAGE_RELEASE_INFO => Some("SWAPCHAIN_IMAGE_RELEASE_INFO"),
            Self::ACTION_STATE_GET_INFO => Some("ACTION_STATE_GET_INFO"),
            Self::HAPTIC_ACTION_INFO => Some("HAPTIC_ACTION_INFO"),
            Self::SESSION_ACTION_SETS_ATTACH_INFO => Some("SESSION_ACTION_SETS_ATTACH_INFO"),
            Self::ACTIONS_SYNC_INFO => Some("ACTIONS_SYNC_INFO"),
            Self::BOUND_SOURCES_FOR_ACTION_ENUMERATE_INFO => {
                Some("BOUND_SOURCES_FOR_ACTION_ENUMERATE_INFO")
            }
            Self::INPUT_SOURCE_LOCALIZED_NAME_GET_INFO => {
                Some("INPUT_SOURCE_LOCALIZED_NAME_GET_INFO")
            }
            Self::SPACES_LOCATE_INFO => Some("SPACES_LOCATE_INFO"),
            Self::SPACE_LOCATIONS => Some("SPACE_LOCATIONS"),
            Self::SPACE_VELOCITIES => Some("SPACE_VELOCITIES"),
            Self::COMPOSITION_LAYER_CUBE_KHR => Some("COMPOSITION_LAYER_CUBE_KHR"),
            Self::INSTANCE_CREATE_INFO_ANDROID_KHR => Some("INSTANCE_CREATE_INFO_ANDROID_KHR"),
            Self::COMPOSITION_LAYER_DEPTH_INFO_KHR => Some("COMPOSITION_LAYER_DEPTH_INFO_KHR"),
            Self::VULKAN_SWAPCHAIN_FORMAT_LIST_CREATE_INFO_KHR => {
                Some("VULKAN_SWAPCHAIN_FORMAT_LIST_CREATE_INFO_KHR")
            }
            Self::EVENT_DATA_PERF_SETTINGS_EXT => Some("EVENT_DATA_PERF_SETTINGS_EXT"),
            Self::COMPOSITION_LAYER_CYLINDER_KHR => Some("COMPOSITION_LAYER_CYLINDER_KHR"),
            Self::COMPOSITION_LAYER_EQUIRECT_KHR => Some("COMPOSITION_LAYER_EQUIRECT_KHR"),
            Self::DEBUG_UTILS_OBJECT_NAME_INFO_EXT => Some("DEBUG_UTILS_OBJECT_NAME_INFO_EXT"),
            Self::DEBUG_UTILS_MESSENGER_CALLBACK_DATA_EXT => {
                Some("DEBUG_UTILS_MESSENGER_CALLBACK_DATA_EXT")
            }
            Self::DEBUG_UTILS_MESSENGER_CREATE_INFO_EXT => {
                Some("DEBUG_UTILS_MESSENGER_CREATE_INFO_EXT")
            }
            Self::DEBUG_UTILS_LABEL_EXT => Some("DEBUG_UTILS_LABEL_EXT"),
            Self::GRAPHICS_BINDING_OPENGL_WIN32_KHR => Some("GRAPHICS_BINDING_OPENGL_WIN32_KHR"),
            Self::GRAPHICS_BINDING_OPENGL_XLIB_KHR => Some("GRAPHICS_BINDING_OPENGL_XLIB_KHR"),
            Self::GRAPHICS_BINDING_OPENGL_XCB_KHR => Some("GRAPHICS_BINDING_OPENGL_XCB_KHR"),
            Self::GRAPHICS_BINDING_OPENGL_WAYLAND_KHR => {
                Some("GRAPHICS_BINDING_OPENGL_WAYLAND_KHR")
            }
            Self::SWAPCHAIN_IMAGE_OPENGL_KHR => Some("SWAPCHAIN_IMAGE_OPENGL_KHR"),
            Self::GRAPHICS_REQUIREMENTS_OPENGL_KHR => Some("GRAPHICS_REQUIREMENTS_OPENGL_KHR"),
            Self::GRAPHICS_BINDING_OPENGL_ES_ANDROID_KHR => {
                Some("GRAPHICS_BINDING_OPENGL_ES_ANDROID_KHR")
            }
            Self::SWAPCHAIN_IMAGE_OPENGL_ES_KHR => Some("SWAPCHAIN_IMAGE_OPENGL_ES_KHR"),
            Self::GRAPHICS_REQUIREMENTS_OPENGL_ES_KHR => {
                Some("GRAPHICS_REQUIREMENTS_OPENGL_ES_KHR")
            }
            Self::GRAPHICS_BINDING_VULKAN_KHR => Some("GRAPHICS_BINDING_VULKAN_KHR"),
            Self::SWAPCHAIN_IMAGE_VULKAN_KHR => Some("SWAPCHAIN_IMAGE_VULKAN_KHR"),
            Self::GRAPHICS_REQUIREMENTS_VULKAN_KHR => Some("GRAPHICS_REQUIREMENTS_VULKAN_KHR"),
            Self::GRAPHICS_BINDING_D3D11_KHR => Some("GRAPHICS_BINDING_D3D11_KHR"),
            Self::SWAPCHAIN_IMAGE_D3D11_KHR => Some("SWAPCHAIN_IMAGE_D3D11_KHR"),
            Self::GRAPHICS_REQUIREMENTS_D3D11_KHR => Some("GRAPHICS_REQUIREMENTS_D3D11_KHR"),
            Self::GRAPHICS_BINDING_D3D12_KHR => Some("GRAPHICS_BINDING_D3D12_KHR"),
            Self::SWAPCHAIN_IMAGE_D3D12_KHR => Some("SWAPCHAIN_IMAGE_D3D12_KHR"),
            Self::GRAPHICS_REQUIREMENTS_D3D12_KHR => Some("GRAPHICS_REQUIREMENTS_D3D12_KHR"),
            Self::GRAPHICS_BINDING_METAL_KHR => Some("GRAPHICS_BINDING_METAL_KHR"),
            Self::SWAPCHAIN_IMAGE_METAL_KHR => Some("SWAPCHAIN_IMAGE_METAL_KHR"),
            Self::GRAPHICS_REQUIREMENTS_METAL_KHR => Some("GRAPHICS_REQUIREMENTS_METAL_KHR"),
            Self::SYSTEM_EYE_GAZE_INTERACTION_PROPERTIES_EXT => {
                Some("SYSTEM_EYE_GAZE_INTERACTION_PROPERTIES_EXT")
            }
            Self::EYE_GAZE_SAMPLE_TIME_EXT => Some("EYE_GAZE_SAMPLE_TIME_EXT"),
            Self::VISIBILITY_MASK_KHR => Some("VISIBILITY_MASK_KHR"),
            Self::EVENT_DATA_VISIBILITY_MASK_CHANGED_KHR => {
                Some("EVENT_DATA_VISIBILITY_MASK_CHANGED_KHR")
            }
            Self::SESSION_CREATE_INFO_OVERLAY_EXTX => Some("SESSION_CREATE_INFO_OVERLAY_EXTX"),
            Self::EVENT_DATA_MAIN_SESSION_VISIBILITY_CHANGED_EXTX => {
                Some("EVENT_DATA_MAIN_SESSION_VISIBILITY_CHANGED_EXTX")
            }
            Self::COMPOSITION_LAYER_COLOR_SCALE_BIAS_KHR => {
                Some("COMPOSITION_LAYER_COLOR_SCALE_BIAS_KHR")
            }
            Self::SPATIAL_ANCHOR_CREATE_INFO_MSFT => Some("SPATIAL_ANCHOR_CREATE_INFO_MSFT"),
            Self::SPATIAL_ANCHOR_SPACE_CREATE_INFO_MSFT => {
                Some("SPATIAL_ANCHOR_SPACE_CREATE_INFO_MSFT")
            }
            Self::COMPOSITION_LAYER_IMAGE_LAYOUT_FB => Some("COMPOSITION_LAYER_IMAGE_LAYOUT_FB"),
            Self::COMPOSITION_LAYER_ALPHA_BLEND_FB => Some("COMPOSITION_LAYER_ALPHA_BLEND_FB"),
            Self::VIEW_CONFIGURATION_DEPTH_RANGE_EXT => Some("VIEW_CONFIGURATION_DEPTH_RANGE_EXT"),
            Self::GRAPHICS_BINDING_EGL_MNDX => Some("GRAPHICS_BINDING_EGL_MNDX"),
            Self::SPATIAL_GRAPH_NODE_SPACE_CREATE_INFO_MSFT => {
                Some("SPATIAL_GRAPH_NODE_SPACE_CREATE_INFO_MSFT")
            }
            Self::SPATIAL_GRAPH_STATIC_NODE_BINDING_CREATE_INFO_MSFT => {
                Some("SPATIAL_GRAPH_STATIC_NODE_BINDING_CREATE_INFO_MSFT")
            }
            Self::SPATIAL_GRAPH_NODE_BINDING_PROPERTIES_GET_INFO_MSFT => {
                Some("SPATIAL_GRAPH_NODE_BINDING_PROPERTIES_GET_INFO_MSFT")
            }
            Self::SPATIAL_GRAPH_NODE_BINDING_PROPERTIES_MSFT => {
                Some("SPATIAL_GRAPH_NODE_BINDING_PROPERTIES_MSFT")
            }
            Self::SYSTEM_HAND_TRACKING_PROPERTIES_EXT => {
                Some("SYSTEM_HAND_TRACKING_PROPERTIES_EXT")
            }
            Self::HAND_TRACKER_CREATE_INFO_EXT => Some("HAND_TRACKER_CREATE_INFO_EXT"),
            Self::HAND_JOINTS_LOCATE_INFO_EXT => Some("HAND_JOINTS_LOCATE_INFO_EXT"),
            Self::HAND_JOINT_LOCATIONS_EXT => Some("HAND_JOINT_LOCATIONS_EXT"),
            Self::HAND_JOINT_VELOCITIES_EXT => Some("HAND_JOINT_VELOCITIES_EXT"),
            Self::SYSTEM_HAND_TRACKING_MESH_PROPERTIES_MSFT => {
                Some("SYSTEM_HAND_TRACKING_MESH_PROPERTIES_MSFT")
            }
            Self::HAND_MESH_SPACE_CREATE_INFO_MSFT => Some("HAND_MESH_SPACE_CREATE_INFO_MSFT"),
            Self::HAND_MESH_UPDATE_INFO_MSFT => Some("HAND_MESH_UPDATE_INFO_MSFT"),
            Self::HAND_MESH_MSFT => Some("HAND_MESH_MSFT"),
            Self::HAND_POSE_TYPE_INFO_MSFT => Some("HAND_POSE_TYPE_INFO_MSFT"),
            Self::SECONDARY_VIEW_CONFIGURATION_SESSION_BEGIN_INFO_MSFT => {
                Some("SECONDARY_VIEW_CONFIGURATION_SESSION_BEGIN_INFO_MSFT")
            }
            Self::SECONDARY_VIEW_CONFIGURATION_STATE_MSFT => {
                Some("SECONDARY_VIEW_CONFIGURATION_STATE_MSFT")
            }
            Self::SECONDARY_VIEW_CONFIGURATION_FRAME_STATE_MSFT => {
                Some("SECONDARY_VIEW_CONFIGURATION_FRAME_STATE_MSFT")
            }
            Self::SECONDARY_VIEW_CONFIGURATION_FRAME_END_INFO_MSFT => {
                Some("SECONDARY_VIEW_CONFIGURATION_FRAME_END_INFO_MSFT")
            }
            Self::SECONDARY_VIEW_CONFIGURATION_LAYER_INFO_MSFT => {
                Some("SECONDARY_VIEW_CONFIGURATION_LAYER_INFO_MSFT")
            }
            Self::SECONDARY_VIEW_CONFIGURATION_SWAPCHAIN_CREATE_INFO_MSFT => {
                Some("SECONDARY_VIEW_CONFIGURATION_SWAPCHAIN_CREATE_INFO_MSFT")
            }
            Self::CONTROLLER_MODEL_KEY_STATE_MSFT => Some("CONTROLLER_MODEL_KEY_STATE_MSFT"),
            Self::CONTROLLER_MODEL_NODE_PROPERTIES_MSFT => {
                Some("CONTROLLER_MODEL_NODE_PROPERTIES_MSFT")
            }
            Self::CONTROLLER_MODEL_PROPERTIES_MSFT => Some("CONTROLLER_MODEL_PROPERTIES_MSFT"),
            Self::CONTROLLER_MODEL_NODE_STATE_MSFT => Some("CONTROLLER_MODEL_NODE_STATE_MSFT"),
            Self::CONTROLLER_MODEL_STATE_MSFT => Some("CONTROLLER_MODEL_STATE_MSFT"),
            Self::VIEW_CONFIGURATION_VIEW_FOV_EPIC => Some("VIEW_CONFIGURATION_VIEW_FOV_EPIC"),
            Self::HOLOGRAPHIC_WINDOW_ATTACHMENT_MSFT => Some("HOLOGRAPHIC_WINDOW_ATTACHMENT_MSFT"),
            Self::COMPOSITION_LAYER_REPROJECTION_INFO_MSFT => {
                Some("COMPOSITION_LAYER_REPROJECTION_INFO_MSFT")
            }
            Self::COMPOSITION_LAYER_REPROJECTION_PLANE_OVERRIDE_MSFT => {
                Some("COMPOSITION_LAYER_REPROJECTION_PLANE_OVERRIDE_MSFT")
            }
            Self::ANDROID_SURFACE_SWAPCHAIN_CREATE_INFO_FB => {
                Some("ANDROID_SURFACE_SWAPCHAIN_CREATE_INFO_FB")
            }
            Self::COMPOSITION_LAYER_SECURE_CONTENT_FB => {
                Some("COMPOSITION_LAYER_SECURE_CONTENT_FB")
            }
            Self::BODY_TRACKER_CREATE_INFO_FB => Some("BODY_TRACKER_CREATE_INFO_FB"),
            Self::BODY_JOINTS_LOCATE_INFO_FB => Some("BODY_JOINTS_LOCATE_INFO_FB"),
            Self::SYSTEM_BODY_TRACKING_PROPERTIES_FB => Some("SYSTEM_BODY_TRACKING_PROPERTIES_FB"),
            Self::BODY_JOINT_LOCATIONS_FB => Some("BODY_JOINT_LOCATIONS_FB"),
            Self::BODY_SKELETON_FB => Some("BODY_SKELETON_FB"),
            Self::INTERACTION_PROFILE_DPAD_BINDING_EXT => {
                Some("INTERACTION_PROFILE_DPAD_BINDING_EXT")
            }
            Self::INTERACTION_PROFILE_ANALOG_THRESHOLD_VALVE => {
                Some("INTERACTION_PROFILE_ANALOG_THRESHOLD_VALVE")
            }
            Self::HAND_JOINTS_MOTION_RANGE_INFO_EXT => Some("HAND_JOINTS_MOTION_RANGE_INFO_EXT"),
            Self::LOADER_INIT_INFO_ANDROID_KHR => Some("LOADER_INIT_INFO_ANDROID_KHR"),
            Self::VULKAN_INSTANCE_CREATE_INFO_KHR => Some("VULKAN_INSTANCE_CREATE_INFO_KHR"),
            Self::VULKAN_DEVICE_CREATE_INFO_KHR => Some("VULKAN_DEVICE_CREATE_INFO_KHR"),
            Self::VULKAN_GRAPHICS_DEVICE_GET_INFO_KHR => {
                Some("VULKAN_GRAPHICS_DEVICE_GET_INFO_KHR")
            }
            Self::COMPOSITION_LAYER_EQUIRECT2_KHR => Some("COMPOSITION_LAYER_EQUIRECT2_KHR"),
            Self::SCENE_OBSERVER_CREATE_INFO_MSFT => Some("SCENE_OBSERVER_CREATE_INFO_MSFT"),
            Self::SCENE_CREATE_INFO_MSFT => Some("SCENE_CREATE_INFO_MSFT"),
            Self::NEW_SCENE_COMPUTE_INFO_MSFT => Some("NEW_SCENE_COMPUTE_INFO_MSFT"),
            Self::VISUAL_MESH_COMPUTE_LOD_INFO_MSFT => Some("VISUAL_MESH_COMPUTE_LOD_INFO_MSFT"),
            Self::SCENE_COMPONENTS_MSFT => Some("SCENE_COMPONENTS_MSFT"),
            Self::SCENE_COMPONENTS_GET_INFO_MSFT => Some("SCENE_COMPONENTS_GET_INFO_MSFT"),
            Self::SCENE_COMPONENT_LOCATIONS_MSFT => Some("SCENE_COMPONENT_LOCATIONS_MSFT"),
            Self::SCENE_COMPONENTS_LOCATE_INFO_MSFT => Some("SCENE_COMPONENTS_LOCATE_INFO_MSFT"),
            Self::SCENE_OBJECTS_MSFT => Some("SCENE_OBJECTS_MSFT"),
            Self::SCENE_COMPONENT_PARENT_FILTER_INFO_MSFT => {
                Some("SCENE_COMPONENT_PARENT_FILTER_INFO_MSFT")
            }
            Self::SCENE_OBJECT_TYPES_FILTER_INFO_MSFT => {
                Some("SCENE_OBJECT_TYPES_FILTER_INFO_MSFT")
            }
            Self::SCENE_PLANES_MSFT => Some("SCENE_PLANES_MSFT"),
            Self::SCENE_PLANE_ALIGNMENT_FILTER_INFO_MSFT => {
                Some("SCENE_PLANE_ALIGNMENT_FILTER_INFO_MSFT")
            }
            Self::SCENE_MESHES_MSFT => Some("SCENE_MESHES_MSFT"),
            Self::SCENE_MESH_BUFFERS_GET_INFO_MSFT => Some("SCENE_MESH_BUFFERS_GET_INFO_MSFT"),
            Self::SCENE_MESH_BUFFERS_MSFT => Some("SCENE_MESH_BUFFERS_MSFT"),
            Self::SCENE_MESH_VERTEX_BUFFER_MSFT => Some("SCENE_MESH_VERTEX_BUFFER_MSFT"),
            Self::SCENE_MESH_INDICES_UINT32_MSFT => Some("SCENE_MESH_INDICES_UINT32_MSFT"),
            Self::SCENE_MESH_INDICES_UINT16_MSFT => Some("SCENE_MESH_INDICES_UINT16_MSFT"),
            Self::SERIALIZED_SCENE_FRAGMENT_DATA_GET_INFO_MSFT => {
                Some("SERIALIZED_SCENE_FRAGMENT_DATA_GET_INFO_MSFT")
            }
            Self::SCENE_DESERIALIZE_INFO_MSFT => Some("SCENE_DESERIALIZE_INFO_MSFT"),
            Self::EVENT_DATA_DISPLAY_REFRESH_RATE_CHANGED_FB => {
                Some("EVENT_DATA_DISPLAY_REFRESH_RATE_CHANGED_FB")
            }
            Self::VIVE_TRACKER_PATHS_HTCX => Some("VIVE_TRACKER_PATHS_HTCX"),
            Self::EVENT_DATA_VIVE_TRACKER_CONNECTED_HTCX => {
                Some("EVENT_DATA_VIVE_TRACKER_CONNECTED_HTCX")
            }
            Self::SYSTEM_FACIAL_TRACKING_PROPERTIES_HTC => {
                Some("SYSTEM_FACIAL_TRACKING_PROPERTIES_HTC")
            }
            Self::FACIAL_TRACKER_CREATE_INFO_HTC => Some("FACIAL_TRACKER_CREATE_INFO_HTC"),
            Self::FACIAL_EXPRESSIONS_HTC => Some("FACIAL_EXPRESSIONS_HTC"),
            Self::SYSTEM_COLOR_SPACE_PROPERTIES_FB => Some("SYSTEM_COLOR_SPACE_PROPERTIES_FB"),
            Self::HAND_TRACKING_MESH_FB => Some("HAND_TRACKING_MESH_FB"),
            Self::HAND_TRACKING_SCALE_FB => Some("HAND_TRACKING_SCALE_FB"),
            Self::HAND_TRACKING_AIM_STATE_FB => Some("HAND_TRACKING_AIM_STATE_FB"),
            Self::HAND_TRACKING_CAPSULES_STATE_FB => Some("HAND_TRACKING_CAPSULES_STATE_FB"),
            Self::SYSTEM_SPATIAL_ENTITY_PROPERTIES_FB => {
                Some("SYSTEM_SPATIAL_ENTITY_PROPERTIES_FB")
            }
            Self::SPATIAL_ANCHOR_CREATE_INFO_FB => Some("SPATIAL_ANCHOR_CREATE_INFO_FB"),
            Self::SPACE_COMPONENT_STATUS_SET_INFO_FB => Some("SPACE_COMPONENT_STATUS_SET_INFO_FB"),
            Self::SPACE_COMPONENT_STATUS_FB => Some("SPACE_COMPONENT_STATUS_FB"),
            Self::EVENT_DATA_SPATIAL_ANCHOR_CREATE_COMPLETE_FB => {
                Some("EVENT_DATA_SPATIAL_ANCHOR_CREATE_COMPLETE_FB")
            }
            Self::EVENT_DATA_SPACE_SET_STATUS_COMPLETE_FB => {
                Some("EVENT_DATA_SPACE_SET_STATUS_COMPLETE_FB")
            }
            Self::FOVEATION_PROFILE_CREATE_INFO_FB => Some("FOVEATION_PROFILE_CREATE_INFO_FB"),
            Self::SWAPCHAIN_CREATE_INFO_FOVEATION_FB => Some("SWAPCHAIN_CREATE_INFO_FOVEATION_FB"),
            Self::SWAPCHAIN_STATE_FOVEATION_FB => Some("SWAPCHAIN_STATE_FOVEATION_FB"),
            Self::FOVEATION_LEVEL_PROFILE_CREATE_INFO_FB => {
                Some("FOVEATION_LEVEL_PROFILE_CREATE_INFO_FB")
            }
            Self::KEYBOARD_SPACE_CREATE_INFO_FB => Some("KEYBOARD_SPACE_CREATE_INFO_FB"),
            Self::KEYBOARD_TRACKING_QUERY_FB => Some("KEYBOARD_TRACKING_QUERY_FB"),
            Self::SYSTEM_KEYBOARD_TRACKING_PROPERTIES_FB => {
                Some("SYSTEM_KEYBOARD_TRACKING_PROPERTIES_FB")
            }
            Self::TRIANGLE_MESH_CREATE_INFO_FB => Some("TRIANGLE_MESH_CREATE_INFO_FB"),
            Self::SYSTEM_PASSTHROUGH_PROPERTIES_FB => Some("SYSTEM_PASSTHROUGH_PROPERTIES_FB"),
            Self::PASSTHROUGH_CREATE_INFO_FB => Some("PASSTHROUGH_CREATE_INFO_FB"),
            Self::PASSTHROUGH_LAYER_CREATE_INFO_FB => Some("PASSTHROUGH_LAYER_CREATE_INFO_FB"),
            Self::COMPOSITION_LAYER_PASSTHROUGH_FB => Some("COMPOSITION_LAYER_PASSTHROUGH_FB"),
            Self::GEOMETRY_INSTANCE_CREATE_INFO_FB => Some("GEOMETRY_INSTANCE_CREATE_INFO_FB"),
            Self::GEOMETRY_INSTANCE_TRANSFORM_FB => Some("GEOMETRY_INSTANCE_TRANSFORM_FB"),
            Self::SYSTEM_PASSTHROUGH_PROPERTIES2_FB => Some("SYSTEM_PASSTHROUGH_PROPERTIES2_FB"),
            Self::PASSTHROUGH_STYLE_FB => Some("PASSTHROUGH_STYLE_FB"),
            Self::PASSTHROUGH_COLOR_MAP_MONO_TO_RGBA_FB => {
                Some("PASSTHROUGH_COLOR_MAP_MONO_TO_RGBA_FB")
            }
            Self::PASSTHROUGH_COLOR_MAP_MONO_TO_MONO_FB => {
                Some("PASSTHROUGH_COLOR_MAP_MONO_TO_MONO_FB")
            }
            Self::PASSTHROUGH_BRIGHTNESS_CONTRAST_SATURATION_FB => {
                Some("PASSTHROUGH_BRIGHTNESS_CONTRAST_SATURATION_FB")
            }
            Self::EVENT_DATA_PASSTHROUGH_STATE_CHANGED_FB => {
                Some("EVENT_DATA_PASSTHROUGH_STATE_CHANGED_FB")
            }
            Self::RENDER_MODEL_PATH_INFO_FB => Some("RENDER_MODEL_PATH_INFO_FB"),
            Self::RENDER_MODEL_PROPERTIES_FB => Some("RENDER_MODEL_PROPERTIES_FB"),
            Self::RENDER_MODEL_BUFFER_FB => Some("RENDER_MODEL_BUFFER_FB"),
            Self::RENDER_MODEL_LOAD_INFO_FB => Some("RENDER_MODEL_LOAD_INFO_FB"),
            Self::SYSTEM_RENDER_MODEL_PROPERTIES_FB => Some("SYSTEM_RENDER_MODEL_PROPERTIES_FB"),
            Self::RENDER_MODEL_CAPABILITIES_REQUEST_FB => {
                Some("RENDER_MODEL_CAPABILITIES_REQUEST_FB")
            }
            Self::BINDING_MODIFICATIONS_KHR => Some("BINDING_MODIFICATIONS_KHR"),
            Self::VIEW_LOCATE_FOVEATED_RENDERING_VARJO => {
                Some("VIEW_LOCATE_FOVEATED_RENDERING_VARJO")
            }
            Self::FOVEATED_VIEW_CONFIGURATION_VIEW_VARJO => {
                Some("FOVEATED_VIEW_CONFIGURATION_VIEW_VARJO")
            }
            Self::SYSTEM_FOVEATED_RENDERING_PROPERTIES_VARJO => {
                Some("SYSTEM_FOVEATED_RENDERING_PROPERTIES_VARJO")
            }
            Self::COMPOSITION_LAYER_DEPTH_TEST_VARJO => Some("COMPOSITION_LAYER_DEPTH_TEST_VARJO"),
            Self::SYSTEM_MARKER_TRACKING_PROPERTIES_VARJO => {
                Some("SYSTEM_MARKER_TRACKING_PROPERTIES_VARJO")
            }
            Self::EVENT_DATA_MARKER_TRACKING_UPDATE_VARJO => {
                Some("EVENT_DATA_MARKER_TRACKING_UPDATE_VARJO")
            }
            Self::MARKER_SPACE_CREATE_INFO_VARJO => Some("MARKER_SPACE_CREATE_INFO_VARJO"),
            Self::FRAME_END_INFO_ML => Some("FRAME_END_INFO_ML"),
            Self::GLOBAL_DIMMER_FRAME_END_INFO_ML => Some("GLOBAL_DIMMER_FRAME_END_INFO_ML"),
            Self::COORDINATE_SPACE_CREATE_INFO_ML => Some("COORDINATE_SPACE_CREATE_INFO_ML"),
            Self::SYSTEM_MARKER_UNDERSTANDING_PROPERTIES_ML => {
                Some("SYSTEM_MARKER_UNDERSTANDING_PROPERTIES_ML")
            }
            Self::MARKER_DETECTOR_CREATE_INFO_ML => Some("MARKER_DETECTOR_CREATE_INFO_ML"),
            Self::MARKER_DETECTOR_ARUCO_INFO_ML => Some("MARKER_DETECTOR_ARUCO_INFO_ML"),
            Self::MARKER_DETECTOR_SIZE_INFO_ML => Some("MARKER_DETECTOR_SIZE_INFO_ML"),
            Self::MARKER_DETECTOR_APRIL_TAG_INFO_ML => Some("MARKER_DETECTOR_APRIL_TAG_INFO_ML"),
            Self::MARKER_DETECTOR_CUSTOM_PROFILE_INFO_ML => {
                Some("MARKER_DETECTOR_CUSTOM_PROFILE_INFO_ML")
            }
            Self::MARKER_DETECTOR_SNAPSHOT_INFO_ML => Some("MARKER_DETECTOR_SNAPSHOT_INFO_ML"),
            Self::MARKER_DETECTOR_STATE_ML => Some("MARKER_DETECTOR_STATE_ML"),
            Self::MARKER_SPACE_CREATE_INFO_ML => Some("MARKER_SPACE_CREATE_INFO_ML"),
            Self::LOCALIZATION_MAP_ML => Some("LOCALIZATION_MAP_ML"),
            Self::EVENT_DATA_LOCALIZATION_CHANGED_ML => Some("EVENT_DATA_LOCALIZATION_CHANGED_ML"),
            Self::MAP_LOCALIZATION_REQUEST_INFO_ML => Some("MAP_LOCALIZATION_REQUEST_INFO_ML"),
            Self::LOCALIZATION_MAP_IMPORT_INFO_ML => Some("LOCALIZATION_MAP_IMPORT_INFO_ML"),
            Self::LOCALIZATION_ENABLE_EVENTS_INFO_ML => Some("LOCALIZATION_ENABLE_EVENTS_INFO_ML"),
            Self::EVENT_DATA_HEADSET_FIT_CHANGED_ML => Some("EVENT_DATA_HEADSET_FIT_CHANGED_ML"),
            Self::EVENT_DATA_EYE_CALIBRATION_CHANGED_ML => {
                Some("EVENT_DATA_EYE_CALIBRATION_CHANGED_ML")
            }
            Self::USER_CALIBRATION_ENABLE_EVENTS_INFO_ML => {
                Some("USER_CALIBRATION_ENABLE_EVENTS_INFO_ML")
            }
            Self::SPATIAL_ANCHOR_PERSISTENCE_INFO_MSFT => {
                Some("SPATIAL_ANCHOR_PERSISTENCE_INFO_MSFT")
            }
            Self::SPATIAL_ANCHOR_FROM_PERSISTED_ANCHOR_CREATE_INFO_MSFT => {
                Some("SPATIAL_ANCHOR_FROM_PERSISTED_ANCHOR_CREATE_INFO_MSFT")
            }
            Self::SCENE_MARKERS_MSFT => Some("SCENE_MARKERS_MSFT"),
            Self::SCENE_MARKER_TYPE_FILTER_MSFT => Some("SCENE_MARKER_TYPE_FILTER_MSFT"),
            Self::SCENE_MARKER_QR_CODES_MSFT => Some("SCENE_MARKER_QR_CODES_MSFT"),
            Self::SPACE_QUERY_INFO_FB => Some("SPACE_QUERY_INFO_FB"),
            Self::SPACE_QUERY_RESULTS_FB => Some("SPACE_QUERY_RESULTS_FB"),
            Self::SPACE_STORAGE_LOCATION_FILTER_INFO_FB => {
                Some("SPACE_STORAGE_LOCATION_FILTER_INFO_FB")
            }
            Self::SPACE_UUID_FILTER_INFO_FB => Some("SPACE_UUID_FILTER_INFO_FB"),
            Self::SPACE_COMPONENT_FILTER_INFO_FB => Some("SPACE_COMPONENT_FILTER_INFO_FB"),
            Self::EVENT_DATA_SPACE_QUERY_RESULTS_AVAILABLE_FB => {
                Some("EVENT_DATA_SPACE_QUERY_RESULTS_AVAILABLE_FB")
            }
            Self::EVENT_DATA_SPACE_QUERY_COMPLETE_FB => Some("EVENT_DATA_SPACE_QUERY_COMPLETE_FB"),
            Self::SPACE_SAVE_INFO_FB => Some("SPACE_SAVE_INFO_FB"),
            Self::SPACE_ERASE_INFO_FB => Some("SPACE_ERASE_INFO_FB"),
            Self::EVENT_DATA_SPACE_SAVE_COMPLETE_FB => Some("EVENT_DATA_SPACE_SAVE_COMPLETE_FB"),
            Self::EVENT_DATA_SPACE_ERASE_COMPLETE_FB => Some("EVENT_DATA_SPACE_ERASE_COMPLETE_FB"),
            Self::SWAPCHAIN_IMAGE_FOVEATION_VULKAN_FB => {
                Some("SWAPCHAIN_IMAGE_FOVEATION_VULKAN_FB")
            }
            Self::SWAPCHAIN_STATE_ANDROID_SURFACE_DIMENSIONS_FB => {
                Some("SWAPCHAIN_STATE_ANDROID_SURFACE_DIMENSIONS_FB")
            }
            Self::SWAPCHAIN_STATE_SAMPLER_OPENGL_ES_FB => {
                Some("SWAPCHAIN_STATE_SAMPLER_OPENGL_ES_FB")
            }
            Self::SWAPCHAIN_STATE_SAMPLER_VULKAN_FB => Some("SWAPCHAIN_STATE_SAMPLER_VULKAN_FB"),
            Self::SPACE_SHARE_INFO_FB => Some("SPACE_SHARE_INFO_FB"),
            Self::EVENT_DATA_SPACE_SHARE_COMPLETE_FB => Some("EVENT_DATA_SPACE_SHARE_COMPLETE_FB"),
            Self::COMPOSITION_LAYER_SPACE_WARP_INFO_FB => {
                Some("COMPOSITION_LAYER_SPACE_WARP_INFO_FB")
            }
            Self::SYSTEM_SPACE_WARP_PROPERTIES_FB => Some("SYSTEM_SPACE_WARP_PROPERTIES_FB"),
            Self::HAPTIC_AMPLITUDE_ENVELOPE_VIBRATION_FB => {
                Some("HAPTIC_AMPLITUDE_ENVELOPE_VIBRATION_FB")
            }
            Self::SEMANTIC_LABELS_FB => Some("SEMANTIC_LABELS_FB"),
            Self::ROOM_LAYOUT_FB => Some("ROOM_LAYOUT_FB"),
            Self::BOUNDARY_2D_FB => Some("BOUNDARY_2D_FB"),
            Self::SEMANTIC_LABELS_SUPPORT_INFO_FB => Some("SEMANTIC_LABELS_SUPPORT_INFO_FB"),
            Self::DIGITAL_LENS_CONTROL_ALMALENCE => Some("DIGITAL_LENS_CONTROL_ALMALENCE"),
            Self::EVENT_DATA_SCENE_CAPTURE_COMPLETE_FB => {
                Some("EVENT_DATA_SCENE_CAPTURE_COMPLETE_FB")
            }
            Self::SCENE_CAPTURE_REQUEST_INFO_FB => Some("SCENE_CAPTURE_REQUEST_INFO_FB"),
            Self::SPACE_CONTAINER_FB => Some("SPACE_CONTAINER_FB"),
            Self::FOVEATION_EYE_TRACKED_PROFILE_CREATE_INFO_META => {
                Some("FOVEATION_EYE_TRACKED_PROFILE_CREATE_INFO_META")
            }
            Self::FOVEATION_EYE_TRACKED_STATE_META => Some("FOVEATION_EYE_TRACKED_STATE_META"),
            Self::SYSTEM_FOVEATION_EYE_TRACKED_PROPERTIES_META => {
                Some("SYSTEM_FOVEATION_EYE_TRACKED_PROPERTIES_META")
            }
            Self::SYSTEM_FACE_TRACKING_PROPERTIES_FB => Some("SYSTEM_FACE_TRACKING_PROPERTIES_FB"),
            Self::FACE_TRACKER_CREATE_INFO_FB => Some("FACE_TRACKER_CREATE_INFO_FB"),
            Self::FACE_EXPRESSION_INFO_FB => Some("FACE_EXPRESSION_INFO_FB"),
            Self::FACE_EXPRESSION_WEIGHTS_FB => Some("FACE_EXPRESSION_WEIGHTS_FB"),
            Self::EYE_TRACKER_CREATE_INFO_FB => Some("EYE_TRACKER_CREATE_INFO_FB"),
            Self::EYE_GAZES_INFO_FB => Some("EYE_GAZES_INFO_FB"),
            Self::EYE_GAZES_FB => Some("EYE_GAZES_FB"),
            Self::SYSTEM_EYE_TRACKING_PROPERTIES_FB => Some("SYSTEM_EYE_TRACKING_PROPERTIES_FB"),
            Self::PASSTHROUGH_KEYBOARD_HANDS_INTENSITY_FB => {
                Some("PASSTHROUGH_KEYBOARD_HANDS_INTENSITY_FB")
            }
            Self::COMPOSITION_LAYER_SETTINGS_FB => Some("COMPOSITION_LAYER_SETTINGS_FB"),
            Self::HAPTIC_PCM_VIBRATION_FB => Some("HAPTIC_PCM_VIBRATION_FB"),
            Self::DEVICE_PCM_SAMPLE_RATE_STATE_FB => Some("DEVICE_PCM_SAMPLE_RATE_STATE_FB"),
            Self::COMPOSITION_LAYER_DEPTH_TEST_FB => Some("COMPOSITION_LAYER_DEPTH_TEST_FB"),
            Self::LOCAL_DIMMING_FRAME_END_INFO_META => Some("LOCAL_DIMMING_FRAME_END_INFO_META"),
            Self::PASSTHROUGH_PREFERENCES_META => Some("PASSTHROUGH_PREFERENCES_META"),
            Self::SYSTEM_VIRTUAL_KEYBOARD_PROPERTIES_META => {
                Some("SYSTEM_VIRTUAL_KEYBOARD_PROPERTIES_META")
            }
            Self::VIRTUAL_KEYBOARD_CREATE_INFO_META => Some("VIRTUAL_KEYBOARD_CREATE_INFO_META"),
            Self::VIRTUAL_KEYBOARD_SPACE_CREATE_INFO_META => {
                Some("VIRTUAL_KEYBOARD_SPACE_CREATE_INFO_META")
            }
            Self::VIRTUAL_KEYBOARD_LOCATION_INFO_META => {
                Some("VIRTUAL_KEYBOARD_LOCATION_INFO_META")
            }
            Self::VIRTUAL_KEYBOARD_MODEL_VISIBILITY_SET_INFO_META => {
                Some("VIRTUAL_KEYBOARD_MODEL_VISIBILITY_SET_INFO_META")
            }
            Self::VIRTUAL_KEYBOARD_ANIMATION_STATE_META => {
                Some("VIRTUAL_KEYBOARD_ANIMATION_STATE_META")
            }
            Self::VIRTUAL_KEYBOARD_MODEL_ANIMATION_STATES_META => {
                Some("VIRTUAL_KEYBOARD_MODEL_ANIMATION_STATES_META")
            }
            Self::VIRTUAL_KEYBOARD_TEXTURE_DATA_META => Some("VIRTUAL_KEYBOARD_TEXTURE_DATA_META"),
            Self::VIRTUAL_KEYBOARD_INPUT_INFO_META => Some("VIRTUAL_KEYBOARD_INPUT_INFO_META"),
            Self::VIRTUAL_KEYBOARD_TEXT_CONTEXT_CHANGE_INFO_META => {
                Some("VIRTUAL_KEYBOARD_TEXT_CONTEXT_CHANGE_INFO_META")
            }
            Self::EVENT_DATA_VIRTUAL_KEYBOARD_COMMIT_TEXT_META => {
                Some("EVENT_DATA_VIRTUAL_KEYBOARD_COMMIT_TEXT_META")
            }
            Self::EVENT_DATA_VIRTUAL_KEYBOARD_BACKSPACE_META => {
                Some("EVENT_DATA_VIRTUAL_KEYBOARD_BACKSPACE_META")
            }
            Self::EVENT_DATA_VIRTUAL_KEYBOARD_ENTER_META => {
                Some("EVENT_DATA_VIRTUAL_KEYBOARD_ENTER_META")
            }
            Self::EVENT_DATA_VIRTUAL_KEYBOARD_SHOWN_META => {
                Some("EVENT_DATA_VIRTUAL_KEYBOARD_SHOWN_META")
            }
            Self::EVENT_DATA_VIRTUAL_KEYBOARD_HIDDEN_META => {
                Some("EVENT_DATA_VIRTUAL_KEYBOARD_HIDDEN_META")
            }
            Self::EXTERNAL_CAMERA_OCULUS => Some("EXTERNAL_CAMERA_OCULUS"),
            Self::VULKAN_SWAPCHAIN_CREATE_INFO_META => Some("VULKAN_SWAPCHAIN_CREATE_INFO_META"),
            Self::PERFORMANCE_METRICS_STATE_META => Some("PERFORMANCE_METRICS_STATE_META"),
            Self::PERFORMANCE_METRICS_COUNTER_META => Some("PERFORMANCE_METRICS_COUNTER_META"),
            Self::SPACE_LIST_SAVE_INFO_FB => Some("SPACE_LIST_SAVE_INFO_FB"),
            Self::EVENT_DATA_SPACE_LIST_SAVE_COMPLETE_FB => {
                Some("EVENT_DATA_SPACE_LIST_SAVE_COMPLETE_FB")
            }
            Self::SPACE_USER_CREATE_INFO_FB => Some("SPACE_USER_CREATE_INFO_FB"),
            Self::SYSTEM_HEADSET_ID_PROPERTIES_META => Some("SYSTEM_HEADSET_ID_PROPERTIES_META"),
            Self::RECOMMENDED_LAYER_RESOLUTION_META => Some("RECOMMENDED_LAYER_RESOLUTION_META"),
            Self::RECOMMENDED_LAYER_RESOLUTION_GET_INFO_META => {
                Some("RECOMMENDED_LAYER_RESOLUTION_GET_INFO_META")
            }
            Self::SYSTEM_PASSTHROUGH_COLOR_LUT_PROPERTIES_META => {
                Some("SYSTEM_PASSTHROUGH_COLOR_LUT_PROPERTIES_META")
            }
            Self::PASSTHROUGH_COLOR_LUT_CREATE_INFO_META => {
                Some("PASSTHROUGH_COLOR_LUT_CREATE_INFO_META")
            }
            Self::PASSTHROUGH_COLOR_LUT_UPDATE_INFO_META => {
                Some("PASSTHROUGH_COLOR_LUT_UPDATE_INFO_META")
            }
            Self::PASSTHROUGH_COLOR_MAP_LUT_META => Some("PASSTHROUGH_COLOR_MAP_LUT_META"),
            Self::PASSTHROUGH_COLOR_MAP_INTERPOLATED_LUT_META => {
                Some("PASSTHROUGH_COLOR_MAP_INTERPOLATED_LUT_META")
            }
            Self::SPACE_TRIANGLE_MESH_GET_INFO_META => Some("SPACE_TRIANGLE_MESH_GET_INFO_META"),
            Self::SPACE_TRIANGLE_MESH_META => Some("SPACE_TRIANGLE_MESH_META"),
            Self::SYSTEM_FACE_TRACKING_PROPERTIES2_FB => {
                Some("SYSTEM_FACE_TRACKING_PROPERTIES2_FB")
            }
            Self::FACE_TRACKER_CREATE_INFO2_FB => Some("FACE_TRACKER_CREATE_INFO2_FB"),
            Self::FACE_EXPRESSION_INFO2_FB => Some("FACE_EXPRESSION_INFO2_FB"),
            Self::FACE_EXPRESSION_WEIGHTS2_FB => Some("FACE_EXPRESSION_WEIGHTS2_FB"),
            Self::ENVIRONMENT_DEPTH_PROVIDER_CREATE_INFO_META => {
                Some("ENVIRONMENT_DEPTH_PROVIDER_CREATE_INFO_META")
            }
            Self::ENVIRONMENT_DEPTH_SWAPCHAIN_CREATE_INFO_META => {
                Some("ENVIRONMENT_DEPTH_SWAPCHAIN_CREATE_INFO_META")
            }
            Self::ENVIRONMENT_DEPTH_SWAPCHAIN_STATE_META => {
                Some("ENVIRONMENT_DEPTH_SWAPCHAIN_STATE_META")
            }
            Self::ENVIRONMENT_DEPTH_IMAGE_ACQUIRE_INFO_META => {
                Some("ENVIRONMENT_DEPTH_IMAGE_ACQUIRE_INFO_META")
            }
            Self::ENVIRONMENT_DEPTH_IMAGE_VIEW_META => Some("ENVIRONMENT_DEPTH_IMAGE_VIEW_META"),
            Self::ENVIRONMENT_DEPTH_IMAGE_META => Some("ENVIRONMENT_DEPTH_IMAGE_META"),
            Self::ENVIRONMENT_DEPTH_HAND_REMOVAL_SET_INFO_META => {
                Some("ENVIRONMENT_DEPTH_HAND_REMOVAL_SET_INFO_META")
            }
            Self::SYSTEM_ENVIRONMENT_DEPTH_PROPERTIES_META => {
                Some("SYSTEM_ENVIRONMENT_DEPTH_PROPERTIES_META")
            }
            Self::PASSTHROUGH_CREATE_INFO_HTC => Some("PASSTHROUGH_CREATE_INFO_HTC"),
            Self::PASSTHROUGH_COLOR_HTC => Some("PASSTHROUGH_COLOR_HTC"),
            Self::PASSTHROUGH_MESH_TRANSFORM_INFO_HTC => {
                Some("PASSTHROUGH_MESH_TRANSFORM_INFO_HTC")
            }
            Self::COMPOSITION_LAYER_PASSTHROUGH_HTC => Some("COMPOSITION_LAYER_PASSTHROUGH_HTC"),
            Self::FOVEATION_APPLY_INFO_HTC => Some("FOVEATION_APPLY_INFO_HTC"),
            Self::FOVEATION_DYNAMIC_MODE_INFO_HTC => Some("FOVEATION_DYNAMIC_MODE_INFO_HTC"),
            Self::FOVEATION_CUSTOM_MODE_INFO_HTC => Some("FOVEATION_CUSTOM_MODE_INFO_HTC"),
            Self::SYSTEM_ANCHOR_PROPERTIES_HTC => Some("SYSTEM_ANCHOR_PROPERTIES_HTC"),
            Self::SPATIAL_ANCHOR_CREATE_INFO_HTC => Some("SPATIAL_ANCHOR_CREATE_INFO_HTC"),
            Self::ACTIVE_ACTION_SET_PRIORITIES_EXT => Some("ACTIVE_ACTION_SET_PRIORITIES_EXT"),
            Self::SYSTEM_FORCE_FEEDBACK_CURL_PROPERTIES_MNDX => {
                Some("SYSTEM_FORCE_FEEDBACK_CURL_PROPERTIES_MNDX")
            }
            Self::FORCE_FEEDBACK_CURL_APPLY_LOCATIONS_MNDX => {
                Some("FORCE_FEEDBACK_CURL_APPLY_LOCATIONS_MNDX")
            }
            Self::HAND_TRACKING_DATA_SOURCE_INFO_EXT => Some("HAND_TRACKING_DATA_SOURCE_INFO_EXT"),
            Self::HAND_TRACKING_DATA_SOURCE_STATE_EXT => {
                Some("HAND_TRACKING_DATA_SOURCE_STATE_EXT")
            }
            Self::PLANE_DETECTOR_CREATE_INFO_EXT => Some("PLANE_DETECTOR_CREATE_INFO_EXT"),
            Self::PLANE_DETECTOR_BEGIN_INFO_EXT => Some("PLANE_DETECTOR_BEGIN_INFO_EXT"),
            Self::PLANE_DETECTOR_GET_INFO_EXT => Some("PLANE_DETECTOR_GET_INFO_EXT"),
            Self::PLANE_DETECTOR_LOCATIONS_EXT => Some("PLANE_DETECTOR_LOCATIONS_EXT"),
            Self::PLANE_DETECTOR_LOCATION_EXT => Some("PLANE_DETECTOR_LOCATION_EXT"),
            Self::PLANE_DETECTOR_POLYGON_BUFFER_EXT => Some("PLANE_DETECTOR_POLYGON_BUFFER_EXT"),
            Self::SYSTEM_PLANE_DETECTION_PROPERTIES_EXT => {
                Some("SYSTEM_PLANE_DETECTION_PROPERTIES_EXT")
            }
            Self::FUTURE_CANCEL_INFO_EXT => Some("FUTURE_CANCEL_INFO_EXT"),
            Self::FUTURE_POLL_INFO_EXT => Some("FUTURE_POLL_INFO_EXT"),
            Self::FUTURE_COMPLETION_EXT => Some("FUTURE_COMPLETION_EXT"),
            Self::FUTURE_POLL_RESULT_EXT => Some("FUTURE_POLL_RESULT_EXT"),
            Self::EVENT_DATA_USER_PRESENCE_CHANGED_EXT => {
                Some("EVENT_DATA_USER_PRESENCE_CHANGED_EXT")
            }
            Self::SYSTEM_USER_PRESENCE_PROPERTIES_EXT => {
                Some("SYSTEM_USER_PRESENCE_PROPERTIES_EXT")
            }
            _ => None,
        };
        if let Some(name) = name {
            fmt.pad(name)
        } else {
            write!(fmt, "unknown structure type (code {})", self.0)
        }
    }
}

#[repr(transparent)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct XrResult(i32);
impl XrResult {
    pub const SUCCESS: XrResult = Self(0i32);
    pub const TIMEOUT_EXPIRED: XrResult = Self(1i32);
    pub const SESSION_LOSS_PENDING: XrResult = Self(3i32);
    pub const EVENT_UNAVAILABLE: XrResult = Self(4i32);
    pub const SPACE_BOUNDS_UNAVAILABLE: XrResult = Self(7i32);
    pub const SESSION_NOT_FOCUSED: XrResult = Self(8i32);
    pub const FRAME_DISCARDED: XrResult = Self(9i32);
    pub const ERROR_VALIDATION_FAILURE: XrResult = Self(-1i32);
    pub const ERROR_RUNTIME_FAILURE: XrResult = Self(-2i32);
    pub const ERROR_OUT_OF_MEMORY: XrResult = Self(-3i32);
    pub const ERROR_API_VERSION_UNSUPPORTED: XrResult = Self(-4i32);
    pub const ERROR_INITIALIZATION_FAILED: XrResult = Self(-6i32);
    pub const ERROR_FUNCTION_UNSUPPORTED: XrResult = Self(-7i32);
    pub const ERROR_FEATURE_UNSUPPORTED: XrResult = Self(-8i32);
    pub const ERROR_EXTENSION_NOT_PRESENT: XrResult = Self(-9i32);
    pub const ERROR_LIMIT_REACHED: XrResult = Self(-10i32);
    pub const ERROR_SIZE_INSUFFICIENT: XrResult = Self(-11i32);
    pub const ERROR_HANDLE_INVALID: XrResult = Self(-12i32);
    pub const ERROR_INSTANCE_LOST: XrResult = Self(-13i32);
    pub const ERROR_SESSION_RUNNING: XrResult = Self(-14i32);
    pub const ERROR_SESSION_NOT_RUNNING: XrResult = Self(-16i32);
    pub const ERROR_SESSION_LOST: XrResult = Self(-17i32);
    pub const ERROR_SYSTEM_INVALID: XrResult = Self(-18i32);
    pub const ERROR_PATH_INVALID: XrResult = Self(-19i32);
    pub const ERROR_PATH_COUNT_EXCEEDED: XrResult = Self(-20i32);
    pub const ERROR_PATH_FORMAT_INVALID: XrResult = Self(-21i32);
    pub const ERROR_PATH_UNSUPPORTED: XrResult = Self(-22i32);
    pub const ERROR_LAYER_INVALID: XrResult = Self(-23i32);
    pub const ERROR_LAYER_LIMIT_EXCEEDED: XrResult = Self(-24i32);
    pub const ERROR_SWAPCHAIN_RECT_INVALID: XrResult = Self(-25i32);
    pub const ERROR_SWAPCHAIN_FORMAT_UNSUPPORTED: XrResult = Self(-26i32);
    pub const ERROR_ACTION_TYPE_MISMATCH: XrResult = Self(-27i32);
    pub const ERROR_SESSION_NOT_READY: XrResult = Self(-28i32);
    pub const ERROR_SESSION_NOT_STOPPING: XrResult = Self(-29i32);
    pub const ERROR_TIME_INVALID: XrResult = Self(-30i32);
    pub const ERROR_REFERENCE_SPACE_UNSUPPORTED: XrResult = Self(-31i32);
    pub const ERROR_FILE_ACCESS_ERROR: XrResult = Self(-32i32);
    pub const ERROR_FILE_CONTENTS_INVALID: XrResult = Self(-33i32);
    pub const ERROR_FORM_FACTOR_UNSUPPORTED: XrResult = Self(-34i32);
    pub const ERROR_FORM_FACTOR_UNAVAILABLE: XrResult = Self(-35i32);
    pub const ERROR_API_LAYER_NOT_PRESENT: XrResult = Self(-36i32);
    pub const ERROR_CALL_ORDER_INVALID: XrResult = Self(-37i32);
    pub const ERROR_GRAPHICS_DEVICE_INVALID: XrResult = Self(-38i32);
    pub const ERROR_POSE_INVALID: XrResult = Self(-39i32);
    pub const ERROR_INDEX_OUT_OF_RANGE: XrResult = Self(-40i32);
    pub const ERROR_VIEW_CONFIGURATION_TYPE_UNSUPPORTED: XrResult = Self(-41i32);
    pub const ERROR_ENVIRONMENT_BLEND_MODE_UNSUPPORTED: XrResult = Self(-42i32);
    pub const ERROR_NAME_DUPLICATED: XrResult = Self(-44i32);
    pub const ERROR_NAME_INVALID: XrResult = Self(-45i32);
    pub const ERROR_ACTIONSET_NOT_ATTACHED: XrResult = Self(-46i32);
    pub const ERROR_ACTIONSETS_ALREADY_ATTACHED: XrResult = Self(-47i32);
    pub const ERROR_LOCALIZED_NAME_DUPLICATED: XrResult = Self(-48i32);
    pub const ERROR_LOCALIZED_NAME_INVALID: XrResult = Self(-49i32);
    pub const ERROR_GRAPHICS_REQUIREMENTS_CALL_MISSING: XrResult = Self(-50i32);
    pub const ERROR_RUNTIME_UNAVAILABLE: XrResult = Self(-51i32);
    pub const ERROR_EXTENSION_DEPENDENCY_NOT_ENABLED: XrResult = Self(-1000710001i32);
    pub const ERROR_PERMISSION_INSUFFICIENT: XrResult = Self(-1000710000i32);
    pub const ERROR_ANDROID_THREAD_SETTINGS_ID_INVALID_KHR: XrResult = Self(-1000003000i32);
    pub const ERROR_ANDROID_THREAD_SETTINGS_FAILURE_KHR: XrResult = Self(-1000003001i32);
    pub const ERROR_CREATE_SPATIAL_ANCHOR_FAILED_MSFT: XrResult = Self(-1000039001i32);
    pub const ERROR_SECONDARY_VIEW_CONFIGURATION_TYPE_NOT_ENABLED_MSFT: XrResult =
        Self(-1000053000i32);
    pub const ERROR_CONTROLLER_MODEL_KEY_INVALID_MSFT: XrResult = Self(-1000055000i32);
    pub const ERROR_REPROJECTION_MODE_UNSUPPORTED_MSFT: XrResult = Self(-1000066000i32);
    pub const ERROR_COMPUTE_NEW_SCENE_NOT_COMPLETED_MSFT: XrResult = Self(-1000097000i32);
    pub const ERROR_SCENE_COMPONENT_ID_INVALID_MSFT: XrResult = Self(-1000097001i32);
    pub const ERROR_SCENE_COMPONENT_TYPE_MISMATCH_MSFT: XrResult = Self(-1000097002i32);
    pub const ERROR_SCENE_MESH_BUFFER_ID_INVALID_MSFT: XrResult = Self(-1000097003i32);
    pub const ERROR_SCENE_COMPUTE_FEATURE_INCOMPATIBLE_MSFT: XrResult = Self(-1000097004i32);
    pub const ERROR_SCENE_COMPUTE_CONSISTENCY_MISMATCH_MSFT: XrResult = Self(-1000097005i32);
    pub const ERROR_DISPLAY_REFRESH_RATE_UNSUPPORTED_FB: XrResult = Self(-1000101000i32);
    pub const ERROR_COLOR_SPACE_UNSUPPORTED_FB: XrResult = Self(-1000108000i32);
    pub const ERROR_SPACE_COMPONENT_NOT_SUPPORTED_FB: XrResult = Self(-1000113000i32);
    pub const ERROR_SPACE_COMPONENT_NOT_ENABLED_FB: XrResult = Self(-1000113001i32);
    pub const ERROR_SPACE_COMPONENT_STATUS_PENDING_FB: XrResult = Self(-1000113002i32);
    pub const ERROR_SPACE_COMPONENT_STATUS_ALREADY_SET_FB: XrResult = Self(-1000113003i32);
    pub const ERROR_UNEXPECTED_STATE_PASSTHROUGH_FB: XrResult = Self(-1000118000i32);
    pub const ERROR_FEATURE_ALREADY_CREATED_PASSTHROUGH_FB: XrResult = Self(-1000118001i32);
    pub const ERROR_FEATURE_REQUIRED_PASSTHROUGH_FB: XrResult = Self(-1000118002i32);
    pub const ERROR_NOT_PERMITTED_PASSTHROUGH_FB: XrResult = Self(-1000118003i32);
    pub const ERROR_INSUFFICIENT_RESOURCES_PASSTHROUGH_FB: XrResult = Self(-1000118004i32);
    pub const ERROR_UNKNOWN_PASSTHROUGH_FB: XrResult = Self(-1000118050i32);
    pub const ERROR_RENDER_MODEL_KEY_INVALID_FB: XrResult = Self(-1000119000i32);
    pub const RENDER_MODEL_UNAVAILABLE_FB: XrResult = Self(1000119020i32);
    pub const ERROR_MARKER_NOT_TRACKED_VARJO: XrResult = Self(-1000124000i32);
    pub const ERROR_MARKER_ID_INVALID_VARJO: XrResult = Self(-1000124001i32);
    pub const ERROR_MARKER_DETECTOR_PERMISSION_DENIED_ML: XrResult = Self(-1000138000i32);
    pub const ERROR_MARKER_DETECTOR_LOCATE_FAILED_ML: XrResult = Self(-1000138001i32);
    pub const ERROR_MARKER_DETECTOR_INVALID_DATA_QUERY_ML: XrResult = Self(-1000138002i32);
    pub const ERROR_MARKER_DETECTOR_INVALID_CREATE_INFO_ML: XrResult = Self(-1000138003i32);
    pub const ERROR_MARKER_INVALID_ML: XrResult = Self(-1000138004i32);
    pub const ERROR_LOCALIZATION_MAP_INCOMPATIBLE_ML: XrResult = Self(-1000139000i32);
    pub const ERROR_LOCALIZATION_MAP_UNAVAILABLE_ML: XrResult = Self(-1000139001i32);
    pub const ERROR_LOCALIZATION_MAP_FAIL_ML: XrResult = Self(-1000139002i32);
    pub const ERROR_LOCALIZATION_MAP_IMPORT_EXPORT_PERMISSION_DENIED_ML: XrResult =
        Self(-1000139003i32);
    pub const ERROR_LOCALIZATION_MAP_PERMISSION_DENIED_ML: XrResult = Self(-1000139004i32);
    pub const ERROR_LOCALIZATION_MAP_ALREADY_EXISTS_ML: XrResult = Self(-1000139005i32);
    pub const ERROR_LOCALIZATION_MAP_CANNOT_EXPORT_CLOUD_MAP_ML: XrResult = Self(-1000139006i32);
    pub const ERROR_SPATIAL_ANCHOR_NAME_NOT_FOUND_MSFT: XrResult = Self(-1000142001i32);
    pub const ERROR_SPATIAL_ANCHOR_NAME_INVALID_MSFT: XrResult = Self(-1000142002i32);
    pub const SCENE_MARKER_DATA_NOT_STRING_MSFT: XrResult = Self(1000147000i32);
    pub const ERROR_SPACE_MAPPING_INSUFFICIENT_FB: XrResult = Self(-1000169000i32);
    pub const ERROR_SPACE_LOCALIZATION_FAILED_FB: XrResult = Self(-1000169001i32);
    pub const ERROR_SPACE_NETWORK_TIMEOUT_FB: XrResult = Self(-1000169002i32);
    pub const ERROR_SPACE_NETWORK_REQUEST_FAILED_FB: XrResult = Self(-1000169003i32);
    pub const ERROR_SPACE_CLOUD_STORAGE_DISABLED_FB: XrResult = Self(-1000169004i32);
    pub const ERROR_PASSTHROUGH_COLOR_LUT_BUFFER_SIZE_MISMATCH_META: XrResult =
        Self(-1000266000i32);
    pub const ENVIRONMENT_DEPTH_NOT_AVAILABLE_META: XrResult = Self(1000291000i32);
    pub const ERROR_HINT_ALREADY_SET_QCOM: XrResult = Self(-1000306000i32);
    pub const ERROR_NOT_AN_ANCHOR_HTC: XrResult = Self(-1000319000i32);
    pub const ERROR_SPACE_NOT_LOCATABLE_EXT: XrResult = Self(-1000429000i32);
    pub const ERROR_PLANE_DETECTION_PERMISSION_DENIED_EXT: XrResult = Self(-1000429001i32);
    pub const ERROR_FUTURE_PENDING_EXT: XrResult = Self(-1000469001i32);
    pub const ERROR_FUTURE_INVALID_EXT: XrResult = Self(-1000469002i32);
    pub const ERROR_EXTENSION_DEPENDENCY_NOT_ENABLED_KHR: XrResult =
        Self::ERROR_EXTENSION_DEPENDENCY_NOT_ENABLED;
    pub const ERROR_PERMISSION_INSUFFICIENT_KHR: XrResult = Self::ERROR_PERMISSION_INSUFFICIENT;
    pub fn from_raw(x: i32) -> Self {
        Self(x)
    }
    pub fn into_raw(self) -> i32 {
        self.0
    }
}

impl std::fmt::Debug for XrResult {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        let reason = match *self {
            Self::SUCCESS => Some("SUCCESS"),
            Self::TIMEOUT_EXPIRED => Some("TIMEOUT_EXPIRED"),
            Self::SESSION_LOSS_PENDING => Some("SESSION_LOSS_PENDING"),
            Self::EVENT_UNAVAILABLE => Some("EVENT_UNAVAILABLE"),
            Self::SPACE_BOUNDS_UNAVAILABLE => Some("SPACE_BOUNDS_UNAVAILABLE"),
            Self::SESSION_NOT_FOCUSED => Some("SESSION_NOT_FOCUSED"),
            Self::FRAME_DISCARDED => Some("FRAME_DISCARDED"),
            Self::ERROR_VALIDATION_FAILURE => Some("ERROR_VALIDATION_FAILURE"),
            Self::ERROR_RUNTIME_FAILURE => Some("ERROR_RUNTIME_FAILURE"),
            Self::ERROR_OUT_OF_MEMORY => Some("ERROR_OUT_OF_MEMORY"),
            Self::ERROR_API_VERSION_UNSUPPORTED => Some("ERROR_API_VERSION_UNSUPPORTED"),
            Self::ERROR_INITIALIZATION_FAILED => Some("ERROR_INITIALIZATION_FAILED"),
            Self::ERROR_FUNCTION_UNSUPPORTED => Some("ERROR_FUNCTION_UNSUPPORTED"),
            Self::ERROR_FEATURE_UNSUPPORTED => Some("ERROR_FEATURE_UNSUPPORTED"),
            Self::ERROR_EXTENSION_NOT_PRESENT => Some("ERROR_EXTENSION_NOT_PRESENT"),
            Self::ERROR_LIMIT_REACHED => Some("ERROR_LIMIT_REACHED"),
            Self::ERROR_SIZE_INSUFFICIENT => Some("ERROR_SIZE_INSUFFICIENT"),
            Self::ERROR_HANDLE_INVALID => Some("ERROR_HANDLE_INVALID"),
            Self::ERROR_INSTANCE_LOST => Some("ERROR_INSTANCE_LOST"),
            Self::ERROR_SESSION_RUNNING => Some("ERROR_SESSION_RUNNING"),
            Self::ERROR_SESSION_NOT_RUNNING => Some("ERROR_SESSION_NOT_RUNNING"),
            Self::ERROR_SESSION_LOST => Some("ERROR_SESSION_LOST"),
            Self::ERROR_SYSTEM_INVALID => Some("ERROR_SYSTEM_INVALID"),
            Self::ERROR_PATH_INVALID => Some("ERROR_PATH_INVALID"),
            Self::ERROR_PATH_COUNT_EXCEEDED => Some("ERROR_PATH_COUNT_EXCEEDED"),
            Self::ERROR_PATH_FORMAT_INVALID => Some("ERROR_PATH_FORMAT_INVALID"),
            Self::ERROR_PATH_UNSUPPORTED => Some("ERROR_PATH_UNSUPPORTED"),
            Self::ERROR_LAYER_INVALID => Some("ERROR_LAYER_INVALID"),
            Self::ERROR_LAYER_LIMIT_EXCEEDED => Some("ERROR_LAYER_LIMIT_EXCEEDED"),
            Self::ERROR_SWAPCHAIN_RECT_INVALID => Some("ERROR_SWAPCHAIN_RECT_INVALID"),
            Self::ERROR_SWAPCHAIN_FORMAT_UNSUPPORTED => Some("ERROR_SWAPCHAIN_FORMAT_UNSUPPORTED"),
            Self::ERROR_ACTION_TYPE_MISMATCH => Some("ERROR_ACTION_TYPE_MISMATCH"),
            Self::ERROR_SESSION_NOT_READY => Some("ERROR_SESSION_NOT_READY"),
            Self::ERROR_SESSION_NOT_STOPPING => Some("ERROR_SESSION_NOT_STOPPING"),
            Self::ERROR_TIME_INVALID => Some("ERROR_TIME_INVALID"),
            Self::ERROR_REFERENCE_SPACE_UNSUPPORTED => Some("ERROR_REFERENCE_SPACE_UNSUPPORTED"),
            Self::ERROR_FILE_ACCESS_ERROR => Some("ERROR_FILE_ACCESS_ERROR"),
            Self::ERROR_FILE_CONTENTS_INVALID => Some("ERROR_FILE_CONTENTS_INVALID"),
            Self::ERROR_FORM_FACTOR_UNSUPPORTED => Some("ERROR_FORM_FACTOR_UNSUPPORTED"),
            Self::ERROR_FORM_FACTOR_UNAVAILABLE => Some("ERROR_FORM_FACTOR_UNAVAILABLE"),
            Self::ERROR_API_LAYER_NOT_PRESENT => Some("ERROR_API_LAYER_NOT_PRESENT"),
            Self::ERROR_CALL_ORDER_INVALID => Some("ERROR_CALL_ORDER_INVALID"),
            Self::ERROR_GRAPHICS_DEVICE_INVALID => Some("ERROR_GRAPHICS_DEVICE_INVALID"),
            Self::ERROR_POSE_INVALID => Some("ERROR_POSE_INVALID"),
            Self::ERROR_INDEX_OUT_OF_RANGE => Some("ERROR_INDEX_OUT_OF_RANGE"),
            Self::ERROR_VIEW_CONFIGURATION_TYPE_UNSUPPORTED => {
                Some("ERROR_VIEW_CONFIGURATION_TYPE_UNSUPPORTED")
            }
            Self::ERROR_ENVIRONMENT_BLEND_MODE_UNSUPPORTED => {
                Some("ERROR_ENVIRONMENT_BLEND_MODE_UNSUPPORTED")
            }
            Self::ERROR_NAME_DUPLICATED => Some("ERROR_NAME_DUPLICATED"),
            Self::ERROR_NAME_INVALID => Some("ERROR_NAME_INVALID"),
            Self::ERROR_ACTIONSET_NOT_ATTACHED => Some("ERROR_ACTIONSET_NOT_ATTACHED"),
            Self::ERROR_ACTIONSETS_ALREADY_ATTACHED => Some("ERROR_ACTIONSETS_ALREADY_ATTACHED"),
            Self::ERROR_LOCALIZED_NAME_DUPLICATED => Some("ERROR_LOCALIZED_NAME_DUPLICATED"),
            Self::ERROR_LOCALIZED_NAME_INVALID => Some("ERROR_LOCALIZED_NAME_INVALID"),
            Self::ERROR_GRAPHICS_REQUIREMENTS_CALL_MISSING => {
                Some("ERROR_GRAPHICS_REQUIREMENTS_CALL_MISSING")
            }
            Self::ERROR_RUNTIME_UNAVAILABLE => Some("ERROR_RUNTIME_UNAVAILABLE"),
            Self::ERROR_EXTENSION_DEPENDENCY_NOT_ENABLED => {
                Some("ERROR_EXTENSION_DEPENDENCY_NOT_ENABLED")
            }
            Self::ERROR_PERMISSION_INSUFFICIENT => Some("ERROR_PERMISSION_INSUFFICIENT"),
            Self::ERROR_ANDROID_THREAD_SETTINGS_ID_INVALID_KHR => {
                Some("ERROR_ANDROID_THREAD_SETTINGS_ID_INVALID_KHR")
            }
            Self::ERROR_ANDROID_THREAD_SETTINGS_FAILURE_KHR => {
                Some("ERROR_ANDROID_THREAD_SETTINGS_FAILURE_KHR")
            }
            Self::ERROR_CREATE_SPATIAL_ANCHOR_FAILED_MSFT => {
                Some("ERROR_CREATE_SPATIAL_ANCHOR_FAILED_MSFT")
            }
            Self::ERROR_SECONDARY_VIEW_CONFIGURATION_TYPE_NOT_ENABLED_MSFT => {
                Some("ERROR_SECONDARY_VIEW_CONFIGURATION_TYPE_NOT_ENABLED_MSFT")
            }
            Self::ERROR_CONTROLLER_MODEL_KEY_INVALID_MSFT => {
                Some("ERROR_CONTROLLER_MODEL_KEY_INVALID_MSFT")
            }
            Self::ERROR_REPROJECTION_MODE_UNSUPPORTED_MSFT => {
                Some("ERROR_REPROJECTION_MODE_UNSUPPORTED_MSFT")
            }
            Self::ERROR_COMPUTE_NEW_SCENE_NOT_COMPLETED_MSFT => {
                Some("ERROR_COMPUTE_NEW_SCENE_NOT_COMPLETED_MSFT")
            }
            Self::ERROR_SCENE_COMPONENT_ID_INVALID_MSFT => {
                Some("ERROR_SCENE_COMPONENT_ID_INVALID_MSFT")
            }
            Self::ERROR_SCENE_COMPONENT_TYPE_MISMATCH_MSFT => {
                Some("ERROR_SCENE_COMPONENT_TYPE_MISMATCH_MSFT")
            }
            Self::ERROR_SCENE_MESH_BUFFER_ID_INVALID_MSFT => {
                Some("ERROR_SCENE_MESH_BUFFER_ID_INVALID_MSFT")
            }
            Self::ERROR_SCENE_COMPUTE_FEATURE_INCOMPATIBLE_MSFT => {
                Some("ERROR_SCENE_COMPUTE_FEATURE_INCOMPATIBLE_MSFT")
            }
            Self::ERROR_SCENE_COMPUTE_CONSISTENCY_MISMATCH_MSFT => {
                Some("ERROR_SCENE_COMPUTE_CONSISTENCY_MISMATCH_MSFT")
            }
            Self::ERROR_DISPLAY_REFRESH_RATE_UNSUPPORTED_FB => {
                Some("ERROR_DISPLAY_REFRESH_RATE_UNSUPPORTED_FB")
            }
            Self::ERROR_COLOR_SPACE_UNSUPPORTED_FB => Some("ERROR_COLOR_SPACE_UNSUPPORTED_FB"),
            Self::ERROR_SPACE_COMPONENT_NOT_SUPPORTED_FB => {
                Some("ERROR_SPACE_COMPONENT_NOT_SUPPORTED_FB")
            }
            Self::ERROR_SPACE_COMPONENT_NOT_ENABLED_FB => {
                Some("ERROR_SPACE_COMPONENT_NOT_ENABLED_FB")
            }
            Self::ERROR_SPACE_COMPONENT_STATUS_PENDING_FB => {
                Some("ERROR_SPACE_COMPONENT_STATUS_PENDING_FB")
            }
            Self::ERROR_SPACE_COMPONENT_STATUS_ALREADY_SET_FB => {
                Some("ERROR_SPACE_COMPONENT_STATUS_ALREADY_SET_FB")
            }
            Self::ERROR_UNEXPECTED_STATE_PASSTHROUGH_FB => {
                Some("ERROR_UNEXPECTED_STATE_PASSTHROUGH_FB")
            }
            Self::ERROR_FEATURE_ALREADY_CREATED_PASSTHROUGH_FB => {
                Some("ERROR_FEATURE_ALREADY_CREATED_PASSTHROUGH_FB")
            }
            Self::ERROR_FEATURE_REQUIRED_PASSTHROUGH_FB => {
                Some("ERROR_FEATURE_REQUIRED_PASSTHROUGH_FB")
            }
            Self::ERROR_NOT_PERMITTED_PASSTHROUGH_FB => Some("ERROR_NOT_PERMITTED_PASSTHROUGH_FB"),
            Self::ERROR_INSUFFICIENT_RESOURCES_PASSTHROUGH_FB => {
                Some("ERROR_INSUFFICIENT_RESOURCES_PASSTHROUGH_FB")
            }
            Self::ERROR_UNKNOWN_PASSTHROUGH_FB => Some("ERROR_UNKNOWN_PASSTHROUGH_FB"),
            Self::ERROR_RENDER_MODEL_KEY_INVALID_FB => Some("ERROR_RENDER_MODEL_KEY_INVALID_FB"),
            Self::RENDER_MODEL_UNAVAILABLE_FB => Some("RENDER_MODEL_UNAVAILABLE_FB"),
            Self::ERROR_MARKER_NOT_TRACKED_VARJO => Some("ERROR_MARKER_NOT_TRACKED_VARJO"),
            Self::ERROR_MARKER_ID_INVALID_VARJO => Some("ERROR_MARKER_ID_INVALID_VARJO"),
            Self::ERROR_MARKER_DETECTOR_PERMISSION_DENIED_ML => {
                Some("ERROR_MARKER_DETECTOR_PERMISSION_DENIED_ML")
            }
            Self::ERROR_MARKER_DETECTOR_LOCATE_FAILED_ML => {
                Some("ERROR_MARKER_DETECTOR_LOCATE_FAILED_ML")
            }
            Self::ERROR_MARKER_DETECTOR_INVALID_DATA_QUERY_ML => {
                Some("ERROR_MARKER_DETECTOR_INVALID_DATA_QUERY_ML")
            }
            Self::ERROR_MARKER_DETECTOR_INVALID_CREATE_INFO_ML => {
                Some("ERROR_MARKER_DETECTOR_INVALID_CREATE_INFO_ML")
            }
            Self::ERROR_MARKER_INVALID_ML => Some("ERROR_MARKER_INVALID_ML"),
            Self::ERROR_LOCALIZATION_MAP_INCOMPATIBLE_ML => {
                Some("ERROR_LOCALIZATION_MAP_INCOMPATIBLE_ML")
            }
            Self::ERROR_LOCALIZATION_MAP_UNAVAILABLE_ML => {
                Some("ERROR_LOCALIZATION_MAP_UNAVAILABLE_ML")
            }
            Self::ERROR_LOCALIZATION_MAP_FAIL_ML => Some("ERROR_LOCALIZATION_MAP_FAIL_ML"),
            Self::ERROR_LOCALIZATION_MAP_IMPORT_EXPORT_PERMISSION_DENIED_ML => {
                Some("ERROR_LOCALIZATION_MAP_IMPORT_EXPORT_PERMISSION_DENIED_ML")
            }
            Self::ERROR_LOCALIZATION_MAP_PERMISSION_DENIED_ML => {
                Some("ERROR_LOCALIZATION_MAP_PERMISSION_DENIED_ML")
            }
            Self::ERROR_LOCALIZATION_MAP_ALREADY_EXISTS_ML => {
                Some("ERROR_LOCALIZATION_MAP_ALREADY_EXISTS_ML")
            }
            Self::ERROR_LOCALIZATION_MAP_CANNOT_EXPORT_CLOUD_MAP_ML => {
                Some("ERROR_LOCALIZATION_MAP_CANNOT_EXPORT_CLOUD_MAP_ML")
            }
            Self::ERROR_SPATIAL_ANCHOR_NAME_NOT_FOUND_MSFT => {
                Some("ERROR_SPATIAL_ANCHOR_NAME_NOT_FOUND_MSFT")
            }
            Self::ERROR_SPATIAL_ANCHOR_NAME_INVALID_MSFT => {
                Some("ERROR_SPATIAL_ANCHOR_NAME_INVALID_MSFT")
            }
            Self::SCENE_MARKER_DATA_NOT_STRING_MSFT => Some("SCENE_MARKER_DATA_NOT_STRING_MSFT"),
            Self::ERROR_SPACE_MAPPING_INSUFFICIENT_FB => {
                Some("ERROR_SPACE_MAPPING_INSUFFICIENT_FB")
            }
            Self::ERROR_SPACE_LOCALIZATION_FAILED_FB => Some("ERROR_SPACE_LOCALIZATION_FAILED_FB"),
            Self::ERROR_SPACE_NETWORK_TIMEOUT_FB => Some("ERROR_SPACE_NETWORK_TIMEOUT_FB"),
            Self::ERROR_SPACE_NETWORK_REQUEST_FAILED_FB => {
                Some("ERROR_SPACE_NETWORK_REQUEST_FAILED_FB")
            }
            Self::ERROR_SPACE_CLOUD_STORAGE_DISABLED_FB => {
                Some("ERROR_SPACE_CLOUD_STORAGE_DISABLED_FB")
            }
            Self::ERROR_PASSTHROUGH_COLOR_LUT_BUFFER_SIZE_MISMATCH_META => {
                Some("ERROR_PASSTHROUGH_COLOR_LUT_BUFFER_SIZE_MISMATCH_META")
            }
            Self::ENVIRONMENT_DEPTH_NOT_AVAILABLE_META => {
                Some("ENVIRONMENT_DEPTH_NOT_AVAILABLE_META")
            }
            Self::ERROR_HINT_ALREADY_SET_QCOM => Some("ERROR_HINT_ALREADY_SET_QCOM"),
            Self::ERROR_NOT_AN_ANCHOR_HTC => Some("ERROR_NOT_AN_ANCHOR_HTC"),
            Self::ERROR_SPACE_NOT_LOCATABLE_EXT => Some("ERROR_SPACE_NOT_LOCATABLE_EXT"),
            Self::ERROR_PLANE_DETECTION_PERMISSION_DENIED_EXT => {
                Some("ERROR_PLANE_DETECTION_PERMISSION_DENIED_EXT")
            }
            Self::ERROR_FUTURE_PENDING_EXT => Some("ERROR_FUTURE_PENDING_EXT"),
            Self::ERROR_FUTURE_INVALID_EXT => Some("ERROR_FUTURE_INVALID_EXT"),
            _ => None,
        };
        if let Some(reason) = reason {
            fmt.pad(reason)
        } else {
            write!(fmt, "unknown error (code {})", self.0)
        }
    }
}
impl std::fmt::Display for XrResult {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        let reason = match * self { Self :: SUCCESS => Some ("function successfully completed") , Self :: TIMEOUT_EXPIRED => Some ("the specified timeout time occurred before the operation could complete") , Self :: SESSION_LOSS_PENDING => Some ("the session will be lost soon") , Self :: EVENT_UNAVAILABLE => Some ("no event was available") , Self :: SPACE_BOUNDS_UNAVAILABLE => Some ("the space's bounds are not known at the moment") , Self :: SESSION_NOT_FOCUSED => Some ("the session is not in the focused state") , Self :: FRAME_DISCARDED => Some ("a frame has been discarded from composition") , Self :: ERROR_VALIDATION_FAILURE => Some ("the function usage was invalid in some way") , Self :: ERROR_RUNTIME_FAILURE => Some ("the runtime failed to handle the function in an unexpected way that is not covered by another error result") , Self :: ERROR_OUT_OF_MEMORY => Some ("a memory allocation has failed") , Self :: ERROR_API_VERSION_UNSUPPORTED => Some ("the runtime does not support the requested API version") , Self :: ERROR_INITIALIZATION_FAILED => Some ("initialization of object could not be completed") , Self :: ERROR_FUNCTION_UNSUPPORTED => Some ("the requested function was not found or is otherwise unsupported") , Self :: ERROR_FEATURE_UNSUPPORTED => Some ("the requested feature is not supported") , Self :: ERROR_EXTENSION_NOT_PRESENT => Some ("a requested extension is not supported") , Self :: ERROR_LIMIT_REACHED => Some ("the runtime supports no more of the requested resource") , Self :: ERROR_SIZE_INSUFFICIENT => Some ("the supplied size was smaller than required") , Self :: ERROR_HANDLE_INVALID => Some ("a supplied object handle was invalid") , Self :: ERROR_INSTANCE_LOST => Some ("the XrInstance was lost or could not be found. It will need to be destroyed and optionally recreated") , Self :: ERROR_SESSION_RUNNING => Some ("the session is already running") , Self :: ERROR_SESSION_NOT_RUNNING => Some ("the session is not yet running") , Self :: ERROR_SESSION_LOST => Some ("the XrSession was lost. It will need to be destroyed and optionally recreated") , Self :: ERROR_SYSTEM_INVALID => Some ("the provided XrSystemId was invalid") , Self :: ERROR_PATH_INVALID => Some ("the provided XrPath was not valid") , Self :: ERROR_PATH_COUNT_EXCEEDED => Some ("the maximum number of supported semantic paths has been reached") , Self :: ERROR_PATH_FORMAT_INVALID => Some ("the semantic path character format is invalid") , Self :: ERROR_PATH_UNSUPPORTED => Some ("the semantic path is unsupported") , Self :: ERROR_LAYER_INVALID => Some ("the layer was NULL or otherwise invalid") , Self :: ERROR_LAYER_LIMIT_EXCEEDED => Some ("the number of specified layers is greater than the supported number") , Self :: ERROR_SWAPCHAIN_RECT_INVALID => Some ("the image rect was negatively sized or otherwise invalid") , Self :: ERROR_SWAPCHAIN_FORMAT_UNSUPPORTED => Some ("the image format is not supported by the runtime or platform") , Self :: ERROR_ACTION_TYPE_MISMATCH => Some ("the API used to retrieve an action's state does not match the action's type") , Self :: ERROR_SESSION_NOT_READY => Some ("the session is not in the ready state") , Self :: ERROR_SESSION_NOT_STOPPING => Some ("the session is not in the stopping state") , Self :: ERROR_TIME_INVALID => Some ("the provided XrTime was zero, negative, or out of range") , Self :: ERROR_REFERENCE_SPACE_UNSUPPORTED => Some ("the specified reference space is not supported by the runtime or system") , Self :: ERROR_FILE_ACCESS_ERROR => Some ("the file could not be accessed") , Self :: ERROR_FILE_CONTENTS_INVALID => Some ("the file's contents were invalid") , Self :: ERROR_FORM_FACTOR_UNSUPPORTED => Some ("the specified form factor is not supported by the current runtime or platform") , Self :: ERROR_FORM_FACTOR_UNAVAILABLE => Some ("the specified form factor is supported, but the device is currently not available, e.g. not plugged in or powered off") , Self :: ERROR_API_LAYER_NOT_PRESENT => Some ("a requested API layer is not present or could not be loaded") , Self :: ERROR_CALL_ORDER_INVALID => Some ("the call was made without having made a previously required call") , Self :: ERROR_GRAPHICS_DEVICE_INVALID => Some ("the given graphics device is not in a valid state. The graphics device could be lost or initialized without meeting graphics requirements") , Self :: ERROR_POSE_INVALID => Some ("the supplied pose was invalid with respect to the requirements") , Self :: ERROR_INDEX_OUT_OF_RANGE => Some ("the supplied index was outside the range of valid indices") , Self :: ERROR_VIEW_CONFIGURATION_TYPE_UNSUPPORTED => Some ("the specified view configuration type is not supported by the runtime or platform") , Self :: ERROR_ENVIRONMENT_BLEND_MODE_UNSUPPORTED => Some ("the specified environment blend mode is not supported by the runtime or platform") , Self :: ERROR_NAME_DUPLICATED => Some ("the name provided was a duplicate of an already-existing resource") , Self :: ERROR_NAME_INVALID => Some ("the name provided was invalid") , Self :: ERROR_ACTIONSET_NOT_ATTACHED => Some ("a referenced action set is not attached to the session") , Self :: ERROR_ACTIONSETS_ALREADY_ATTACHED => Some ("the session already has attached action sets") , Self :: ERROR_LOCALIZED_NAME_DUPLICATED => Some ("the localized name provided was a duplicate of an already-existing resource") , Self :: ERROR_LOCALIZED_NAME_INVALID => Some ("the localized name provided was invalid") , Self :: ERROR_GRAPHICS_REQUIREMENTS_CALL_MISSING => Some ("the xrGetGraphicsRequirements* call was not made before calling xrCreateSession") , Self :: ERROR_RUNTIME_UNAVAILABLE => Some ("the loader was unable to find or load a runtime") , Self :: ERROR_EXTENSION_DEPENDENCY_NOT_ENABLED => Some ("one or more of the extensions being enabled has dependency on extensions that are not enabled") , Self :: ERROR_PERMISSION_INSUFFICIENT => Some ("insufficient permissions. This error is included for use by vendor extensions. The precise definition of `XR_ERROR_PERMISSION_INSUFFICIENT` and actions possible by the developer or user to resolve it can vary by platform, extension or function. The developer should refer to the documentation of the function that returned the error code and extension it was defined") , Self :: ERROR_ANDROID_THREAD_SETTINGS_ID_INVALID_KHR => Some ("xrSetAndroidApplicationThreadKHR failed as thread id is invalid") , Self :: ERROR_ANDROID_THREAD_SETTINGS_FAILURE_KHR => Some ("xrSetAndroidApplicationThreadKHR failed setting the thread attributes/priority") , Self :: ERROR_CREATE_SPATIAL_ANCHOR_FAILED_MSFT => Some ("spatial anchor could not be created at that location") , Self :: ERROR_SECONDARY_VIEW_CONFIGURATION_TYPE_NOT_ENABLED_MSFT => Some ("the secondary view configuration was not enabled when creating the session") , Self :: ERROR_CONTROLLER_MODEL_KEY_INVALID_MSFT => Some ("the controller model key is invalid") , Self :: ERROR_REPROJECTION_MODE_UNSUPPORTED_MSFT => Some ("the reprojection mode is not supported") , Self :: ERROR_COMPUTE_NEW_SCENE_NOT_COMPLETED_MSFT => Some ("compute new scene not completed") , Self :: ERROR_SCENE_COMPONENT_ID_INVALID_MSFT => Some ("scene component id invalid") , Self :: ERROR_SCENE_COMPONENT_TYPE_MISMATCH_MSFT => Some ("scene component type mismatch") , Self :: ERROR_SCENE_MESH_BUFFER_ID_INVALID_MSFT => Some ("scene mesh buffer id invalid") , Self :: ERROR_SCENE_COMPUTE_FEATURE_INCOMPATIBLE_MSFT => Some ("scene compute feature incompatible") , Self :: ERROR_SCENE_COMPUTE_CONSISTENCY_MISMATCH_MSFT => Some ("scene compute consistency mismatch") , Self :: ERROR_DISPLAY_REFRESH_RATE_UNSUPPORTED_FB => Some ("the display refresh rate is not supported by the platform") , Self :: ERROR_COLOR_SPACE_UNSUPPORTED_FB => Some ("the color space is not supported by the runtime") , Self :: ERROR_SPACE_COMPONENT_NOT_SUPPORTED_FB => Some ("the component type is not supported for this space") , Self :: ERROR_SPACE_COMPONENT_NOT_ENABLED_FB => Some ("the required component is not enabled for this space") , Self :: ERROR_SPACE_COMPONENT_STATUS_PENDING_FB => Some ("a request to set the component's status is currently pending") , Self :: ERROR_SPACE_COMPONENT_STATUS_ALREADY_SET_FB => Some ("the component is already set to the requested value") , Self :: ERROR_UNEXPECTED_STATE_PASSTHROUGH_FB => Some ("the object state is unexpected for the issued command") , Self :: ERROR_FEATURE_ALREADY_CREATED_PASSTHROUGH_FB => Some ("trying to create an MR feature when one was already created and only one instance is allowed") , Self :: ERROR_FEATURE_REQUIRED_PASSTHROUGH_FB => Some ("requested functionality requires a feature to be created first") , Self :: ERROR_NOT_PERMITTED_PASSTHROUGH_FB => Some ("requested functionality is not permitted - application is not allowed to perform the requested operation") , Self :: ERROR_INSUFFICIENT_RESOURCES_PASSTHROUGH_FB => Some ("there were insufficient resources available to perform an operation") , Self :: ERROR_UNKNOWN_PASSTHROUGH_FB => Some ("unknown Passthrough error (no further details provided)") , Self :: ERROR_RENDER_MODEL_KEY_INVALID_FB => Some ("the model key is invalid") , Self :: RENDER_MODEL_UNAVAILABLE_FB => Some ("the model is unavailable") , Self :: ERROR_MARKER_NOT_TRACKED_VARJO => Some ("marker tracking is disabled or the specified marker is not currently tracked") , Self :: ERROR_MARKER_ID_INVALID_VARJO => Some ("the specified marker ID is not valid") , Self :: ERROR_MARKER_DETECTOR_PERMISSION_DENIED_ML => Some ("the com.magicleap.permission.MARKER_TRACKING permission was denied") , Self :: ERROR_MARKER_DETECTOR_LOCATE_FAILED_ML => Some ("the specified marker could not be located spatially") , Self :: ERROR_MARKER_DETECTOR_INVALID_DATA_QUERY_ML => Some ("the marker queried does not contain data of the requested type") , Self :: ERROR_MARKER_DETECTOR_INVALID_CREATE_INFO_ML => Some ("createInfo contains mutually exclusive parameters, such as setting XR_MARKER_DETECTOR_CORNER_REFINE_METHOD_APRIL_TAG_ML with XR_MARKER_TYPE_ARUCO_ML") , Self :: ERROR_MARKER_INVALID_ML => Some ("the marker id passed to the function was invalid") , Self :: ERROR_LOCALIZATION_MAP_INCOMPATIBLE_ML => Some ("the localization map being imported is not compatible with current OS or mode") , Self :: ERROR_LOCALIZATION_MAP_UNAVAILABLE_ML => Some ("the localization map requested is not available") , Self :: ERROR_LOCALIZATION_MAP_FAIL_ML => Some ("the map localization service failed to fulfill the request, retry later") , Self :: ERROR_LOCALIZATION_MAP_IMPORT_EXPORT_PERMISSION_DENIED_ML => Some ("the com.magicleap.permission.SPACE_IMPORT_EXPORT permission was denied") , Self :: ERROR_LOCALIZATION_MAP_PERMISSION_DENIED_ML => Some ("the com.magicleap.permission.SPACE_MANAGER permission was denied") , Self :: ERROR_LOCALIZATION_MAP_ALREADY_EXISTS_ML => Some ("the map being imported already exists in the system") , Self :: ERROR_LOCALIZATION_MAP_CANNOT_EXPORT_CLOUD_MAP_ML => Some ("the map localization service cannot export cloud based maps") , Self :: ERROR_SPATIAL_ANCHOR_NAME_NOT_FOUND_MSFT => Some ("a spatial anchor was not found associated with the spatial anchor name provided") , Self :: ERROR_SPATIAL_ANCHOR_NAME_INVALID_MSFT => Some ("the spatial anchor name provided was not valid") , Self :: SCENE_MARKER_DATA_NOT_STRING_MSFT => Some ("marker does not encode a string") , Self :: ERROR_SPACE_MAPPING_INSUFFICIENT_FB => Some ("anchor import from cloud or export from device failed") , Self :: ERROR_SPACE_LOCALIZATION_FAILED_FB => Some ("anchors were downloaded from the cloud but failed to be imported/aligned on the device") , Self :: ERROR_SPACE_NETWORK_TIMEOUT_FB => Some ("timeout occurred while waiting for network request to complete") , Self :: ERROR_SPACE_NETWORK_REQUEST_FAILED_FB => Some ("the network request failed") , Self :: ERROR_SPACE_CLOUD_STORAGE_DISABLED_FB => Some ("cloud storage is required for this operation but is currently disabled") , Self :: ERROR_PASSTHROUGH_COLOR_LUT_BUFFER_SIZE_MISMATCH_META => Some ("the provided data buffer did not match the required size") , Self :: ENVIRONMENT_DEPTH_NOT_AVAILABLE_META => Some ("warning: The requested depth image is not yet available") , Self :: ERROR_HINT_ALREADY_SET_QCOM => Some ("tracking optimization hint is already set for the domain") , Self :: ERROR_NOT_AN_ANCHOR_HTC => Some ("the provided space is valid but not an anchor") , Self :: ERROR_SPACE_NOT_LOCATABLE_EXT => Some ("the space passed to the function was not locatable") , Self :: ERROR_PLANE_DETECTION_PERMISSION_DENIED_EXT => Some ("the permission for this resource was not granted") , Self :: ERROR_FUTURE_PENDING_EXT => Some ("returned by completion function to indicate future is not ready") , Self :: ERROR_FUTURE_INVALID_EXT => Some ("returned by completion function to indicate future is not valid") , _ => None , } ;
        if let Some(reason) = reason {
            fmt.pad(reason)
        } else {
            write!(fmt, "unknown error (code {})", self.0)
        }
    }
}
