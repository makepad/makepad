#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
use std::os::raw::{c_int, c_char};

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACameraManager {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACameraIdList {
    pub numCameras: c_int,
    pub cameraIds: *mut *const c_char,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACameraMetadata {
    _unused: [u8; 0],
}

pub const ACAMERA_LENS_FACING: u32 = 524293;
pub const ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS: u32 = 851978;
pub const ACAMERA_CONTROL_AE_TARGET_FPS_RANGE: u32 = 65541;
pub const ACAMERA_JPEG_QUALITY:u32 = 458756;

pub const ACAMERA_LENS_FACING_FRONT: u8 = 0;
pub const ACAMERA_LENS_FACING_BACK: u8 = 1;
pub const ACAMERA_LENS_FACING_EXTERNAL: u8 = 2;

pub const AIMAGE_FORMAT_YUV_420_888: u32 = 35;
pub const AIMAGE_FORMAT_JPEG: u32 = 256;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACameraMetadata_rational {
    pub numerator: i32,
    pub denominator: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct ACameraMetadata_const_entry {
    pub tag: u32,
    pub type_: u8,
    pub count: u32,
    pub data: ACameraMetadata_const_entry__bindgen_ty_1,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub union ACameraMetadata_const_entry__bindgen_ty_1 {
    pub u8_: *const u8,
    pub i32_: *const i32,
    pub f: *const f32,
    pub i64_: *const i64,
    pub d: *const f64,
    pub r: *const ACameraMetadata_rational,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACameraDevice {
    _unused: [u8; 0],
}

pub type ACameraDevice_StateCallback = ::std::option::Option<
unsafe extern "C" fn(context: *mut ::std::os::raw::c_void, device: *mut ACameraDevice),
>;

pub type ACameraDevice_ErrorStateCallback = ::std::option::Option<
unsafe extern "C" fn(
    context: *mut ::std::os::raw::c_void,
    device: *mut ACameraDevice,
    error: ::std::os::raw::c_int,
),
>;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACameraDevice_StateCallbacks {
    pub context: *mut ::std::os::raw::c_void,
    pub onDisconnected: ACameraDevice_StateCallback,
    pub onError: ACameraDevice_ErrorStateCallback,
}

pub type ACameraDevice_request_template = ::std::os::raw::c_uint;
pub const TEMPLATE_PREVIEW: ACameraDevice_request_template = 1;
pub const TEMPLATE_STILL_CAPTURE: ACameraDevice_request_template = 2;
pub const TEMPLATE_RECORD: ACameraDevice_request_template = 3;
pub const TEMPLATE_VIDEO_SNAPSHOT: ACameraDevice_request_template = 4;
pub const TEMPLATE_ZERO_SHUTTER_LAG: ACameraDevice_request_template = 5;
pub const TEMPLATE_MANUAL: ACameraDevice_request_template = 6;



#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACaptureRequest {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ANativeWindow {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACaptureSessionOutput {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACaptureSessionOutputContainer {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AImageReader {
    _unused: [u8; 0],
}

type camera_status_t = c_int;

pub type ACameraWindowType = ANativeWindow;
pub type media_status_t = ::std::os::raw::c_int;

pub type AImageReader_ImageCallback = ::std::option::Option<
unsafe extern "C" fn(context: *mut ::std::os::raw::c_void, reader: *mut AImageReader),
>;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AImageReader_ImageListener {
    pub context: *mut ::std::os::raw::c_void,
    pub onImageAvailable: AImageReader_ImageCallback,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACameraOutputTarget {
    _unused: [u8; 0],
}

pub type ACameraCaptureSession_stateCallback = ::std::option::Option<
unsafe extern "C" fn(context: *mut ::std::os::raw::c_void, session: *mut ACameraCaptureSession),
>;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACameraCaptureSession_stateCallbacks {
    pub context: *mut ::std::os::raw::c_void,
    pub onClosed: ACameraCaptureSession_stateCallback,
    pub onReady: ACameraCaptureSession_stateCallback,
    pub onActive: ACameraCaptureSession_stateCallback,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACameraCaptureSession {
    _unused: [u8; 0],
}

pub type ACameraCaptureSession_captureCallback_start = ::std::option::Option<
unsafe extern "C" fn(
    context: *mut ::std::os::raw::c_void,
    session: *mut ACameraCaptureSession,
    request: *const ACaptureRequest,
    timestamp: i64,
),
>;
pub type ACameraCaptureSession_captureCallback_result = ::std::option::Option<
unsafe extern "C" fn(
    context: *mut ::std::os::raw::c_void,
    session: *mut ACameraCaptureSession,
    request: *mut ACaptureRequest,
    result: *const ACameraMetadata,
),
>;
pub type ACameraCaptureSession_captureCallback_failed = ::std::option::Option<
unsafe extern "C" fn(
    context: *mut ::std::os::raw::c_void,
    session: *mut ACameraCaptureSession,
    request: *mut ACaptureRequest,
    failure: *mut ACameraCaptureFailure,
),
>;
pub type ACameraCaptureSession_captureCallback_sequenceEnd = ::std::option::Option<
unsafe extern "C" fn(
    context: *mut ::std::os::raw::c_void,
    session: *mut ACameraCaptureSession,
    sequenceId: ::std::os::raw::c_int,
    frameNumber: i64,
),
>;
pub type ACameraCaptureSession_captureCallback_sequenceAbort = ::std::option::Option<
unsafe extern "C" fn(
    context: *mut ::std::os::raw::c_void,
    session: *mut ACameraCaptureSession,
    sequenceId: ::std::os::raw::c_int,
),
>;
pub type ACameraCaptureSession_captureCallback_bufferLost = ::std::option::Option<
unsafe extern "C" fn(
    context: *mut ::std::os::raw::c_void,
    session: *mut ACameraCaptureSession,
    request: *mut ACaptureRequest,
    window: *mut ACameraWindowType,
    frameNumber: i64,
),
>;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACameraCaptureSession_captureCallbacks {
    pub context: *mut ::std::os::raw::c_void,
    pub onCaptureStarted: ACameraCaptureSession_captureCallback_start,
    pub onCaptureProgressed: ACameraCaptureSession_captureCallback_result,
    pub onCaptureCompleted: ACameraCaptureSession_captureCallback_result,
    pub onCaptureFailed: ACameraCaptureSession_captureCallback_failed,
    pub onCaptureSequenceCompleted: ACameraCaptureSession_captureCallback_sequenceEnd,
    pub onCaptureSequenceAborted: ACameraCaptureSession_captureCallback_sequenceAbort,
    pub onCaptureBufferLost: ACameraCaptureSession_captureCallback_bufferLost,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ACameraCaptureFailure {
    pub frameNumber: i64,
    pub reason: ::std::os::raw::c_int,
    pub sequenceId: ::std::os::raw::c_int,
    pub wasImageCaptured: bool,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AImage {
    _unused: [u8; 0],
}

#[link(name = "nativewindow")]
extern "C" {
    pub fn ANativeWindow_acquire(window: *mut ANativeWindow);
    pub fn ANativeWindow_release(window: *mut ANativeWindow);
}

#[link(name = "mediandk")]
extern "C" {
    
    pub fn AImageReader_new(
        width: i32,
        height: i32,
        format: u32,
        maxImages: i32,
        reader: *mut *mut AImageReader,
    ) -> media_status_t;
    
    pub fn AImageReader_setImageListener(
        reader: *mut AImageReader,
        listener: *mut AImageReader_ImageListener,
    ) -> media_status_t;
    
    pub fn AImageReader_getWindow(
        reader: *mut AImageReader,
        window: *mut *mut ANativeWindow,
    ) -> media_status_t;
    
    pub fn AImageReader_delete(reader: *mut AImageReader);
    
    pub fn AImageReader_acquireNextImage(
        reader: *mut AImageReader,
        image: *mut *mut AImage,
    ) -> media_status_t;
    
    pub fn AImage_getPlaneData(
        image: *const AImage,
        planeIdx: ::std::os::raw::c_int,
        data: *mut *mut u8,
        dataLength: *mut ::std::os::raw::c_int,
    ) -> media_status_t;
    pub fn AImage_delete(image: *mut AImage);
    
}

#[link(name = "camera2ndk")]
extern "C" {
    pub fn ACameraManager_create() -> *mut ACameraManager;
    pub fn ACameraManager_delete(manager: *mut ACameraManager);
    pub fn ACameraManager_getCameraIdList(
        manager: *mut ACameraManager,
        cameraIdList: *mut *mut ACameraIdList,
    ) -> camera_status_t;
    pub fn ACameraManager_getCameraCharacteristics(
        manager: *mut ACameraManager,
        cameraId: *const ::std::os::raw::c_char,
        characteristics: *mut *mut ACameraMetadata,
    ) -> camera_status_t;
    pub fn ACameraMetadata_free(metadata: *mut ACameraMetadata);
    
    pub fn ACameraMetadata_getAllTags(
        metadata: *const ACameraMetadata,
        numEntries: *mut i32,
        tags: *mut *const u32,
    ) -> camera_status_t;
    
    pub fn ACameraMetadata_getConstEntry(
        metadata: *const ACameraMetadata,
        tag: u32,
        entry: *mut ACameraMetadata_const_entry,
    ) -> camera_status_t;
    
    pub fn ACameraManager_deleteCameraIdList(cameraIdList: *mut ACameraIdList);
    
    pub fn ACameraManager_openCamera(
        manager: *mut ACameraManager,
        cameraId: *const ::std::os::raw::c_char,
        callback: *mut ACameraDevice_StateCallbacks,
        device: *mut *mut ACameraDevice,
    ) -> camera_status_t;
    
    pub fn ACameraDevice_createCaptureRequest(
        device: *const ACameraDevice,
        templateId: ACameraDevice_request_template,
        request: *mut *mut ACaptureRequest,
    ) -> camera_status_t;
    
    pub fn ACaptureSessionOutput_create(
        anw: *mut ACameraWindowType,
        output: *mut *mut ACaptureSessionOutput,
    ) -> camera_status_t;
    
    pub fn ACaptureSessionOutputContainer_create(
        container: *mut *mut ACaptureSessionOutputContainer,
    ) -> camera_status_t;
    
    pub fn ACaptureSessionOutputContainer_add(
        container: *mut ACaptureSessionOutputContainer,
        output: *const ACaptureSessionOutput,
    ) -> camera_status_t;
    
    
    pub fn ACameraOutputTarget_create(
        window: *mut ACameraWindowType,
        output: *mut *mut ACameraOutputTarget,
    ) -> camera_status_t;
    
    pub fn ACaptureRequest_addTarget(
        request: *mut ACaptureRequest,
        output: *const ACameraOutputTarget,
    ) -> camera_status_t;
    
    pub fn ACameraDevice_createCaptureSession(
        device: *mut ACameraDevice,
        outputs: *const ACaptureSessionOutputContainer,
        callbacks: *const ACameraCaptureSession_stateCallbacks,
        session: *mut *mut ACameraCaptureSession,
    ) -> camera_status_t;
    
    pub fn ACameraCaptureSession_setRepeatingRequest(
        session: *mut ACameraCaptureSession,
        callbacks: *mut ACameraCaptureSession_captureCallbacks,
        numRequests: ::std::os::raw::c_int,
        requests: *mut *mut ACaptureRequest,
        captureSequenceId: *mut ::std::os::raw::c_int,
    ) -> camera_status_t;
    
    pub fn ACameraCaptureSession_stopRepeating(
        session: *mut ACameraCaptureSession,
    ) -> camera_status_t;
    
    pub fn ACameraCaptureSession_close(session: *mut ACameraCaptureSession);
    
    pub fn ACaptureSessionOutputContainer_free(container: *mut ACaptureSessionOutputContainer);
    
    pub fn ACaptureSessionOutput_free(output: *mut ACaptureSessionOutput);
    
    pub fn ACameraDevice_close(device: *mut ACameraDevice) -> camera_status_t;
    
    pub fn ACaptureRequest_free(request: *mut ACaptureRequest);
    
    pub fn ACameraOutputTarget_free(output: *mut ACameraOutputTarget);
    
    pub fn ACaptureRequest_removeTarget(
        request: *mut ACaptureRequest,
        output: *const ACameraOutputTarget,
    ) -> camera_status_t;
    
    pub fn ACaptureRequest_setEntry_u8(
        request: *mut ACaptureRequest,
        tag: u32,
        count: u32,
        data: *const u8,
    ) -> camera_status_t;
}

