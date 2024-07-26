#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::ffi::c_void;
use makepad_jni_sys as jni_sys;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ANativeWindow {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AAssetManager {
    _unused: [u8; 0],
}


#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AAsset {
    _unused: [u8; 0],
}

pub const AASSET_MODE_BUFFER: ::std::os::raw::c_uint = 3;

#[link(name = "android")]
extern "C" {
    pub fn AAssetManager_open(
        mgr: *mut AAssetManager,
        filename: *const ::std::os::raw::c_char,
        mode: ::std::os::raw::c_int,
    ) -> *mut AAsset;
    pub fn AAsset_getLength64(asset: *mut AAsset) -> i64;
    pub fn ANativeWindow_release(window: *mut ANativeWindow);
    pub fn ANativeWindow_fromSurface(env: *mut jni_sys::JNIEnv, surface: jni_sys::jobject) -> *mut ANativeWindow;
    pub fn AAsset_read(
        asset: *mut AAsset,
        buf: *mut ::std::os::raw::c_void,
        count: usize,
    ) -> ::std::os::raw::c_int;
    pub fn AAsset_close(asset: *mut AAsset);
    pub fn AAssetManager_fromJava(
        env: *mut jni_sys::JNIEnv,
        assetManager: jni_sys::jobject,
    ) -> *mut AAssetManager;
    
    pub fn  ANativeWindow_setFrameRate(
         window: *mut ANativeWindow,
         frameRate:f32,
         compatibility:i8
    )->i32;
}

pub type AChoreographer = c_void;
pub type AChoreographerFrameCallbackData = c_void;

pub type AChoreographer_vsyncCallback = unsafe extern "C" fn(
    callbackData: *mut AChoreographerFrameCallbackData,
    data: *mut c_void,
);

#[repr(C)]
pub struct AChoreographerFrameTimelineInfo {
    pub id: i64,
    pub vsyncId: i64,
    pub expectedPresentationTime: i64,
    pub deadline: i64,
}

extern "C" {
    pub fn AChoreographer_getInstance() -> *mut AChoreographer;

    pub fn AChoreographer_postVsyncCallback(
        choreographer: *mut AChoreographer,
        callback: Option<AChoreographer_vsyncCallback>,
        data: *mut c_void,
    ) -> i32;

    pub fn AChoreographerFrameCallbackData_getFrameTimelinesCount(
        data: *const AChoreographerFrameCallbackData,
    ) -> usize;

    pub fn AChoreographerFrameCallbackData_getFrameTimelineInfo(
        callbackData: *const AChoreographerFrameCallbackData,
        index: usize,
    ) -> AChoreographerFrameTimelineInfo;
}