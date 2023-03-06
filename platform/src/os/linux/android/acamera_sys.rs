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

pub const ACAMERA_LENS_FACING_FRONT: u8 = 0;
pub const ACAMERA_LENS_FACING_BACK: u8 = 1;
pub const ACAMERA_LENS_FACING_EXTERNAL: u8 = 2;

pub const AIMAGE_FORMAT_YUV_420_888:u32 = 35;
pub const AIMAGE_FORMAT_JPEG:u32 = 256;

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

type camera_status_t = c_int;

#[link(name = "camera2ndk")]
extern "C"{
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

}

