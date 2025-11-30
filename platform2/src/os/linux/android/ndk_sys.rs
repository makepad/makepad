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

/// The function type for posting callbacks to the AChoreographer
pub type AChoreographerPostCallbackFn = unsafe extern "C" fn(
    *mut AChoreographer,
    Option<unsafe extern "C" fn(*mut AChoreographerFrameCallbackData, *mut std::ffi::c_void)>,
    *mut std::ffi::c_void,
) -> i32;

#[cfg(not(no_android_choreographer))]
extern "C" {
    pub fn AChoreographer_getInstance() -> *mut AChoreographer;
    // Android SDK >= 33
    pub fn AChoreographer_postVsyncCallback(
        choreographer: *mut AChoreographer,
        callback: Option<AChoreographer_vsyncCallback>,
        data: *mut c_void,
    ) -> i32;

    // Android SDK < 33 && >= 29
    pub fn AChoreographer_postFrameCallback64(
        choreographer: *mut AChoreographer,
        callback: Option<AChoreographer_vsyncCallback>,
        data: *mut c_void,
    ) -> i32;
}

#[repr(C)]
pub struct ANativeActivity {
    pub callbacks: *mut ANativeActivityCallbacks,
    pub vm: *mut jni_sys::JavaVM,
    pub env: *mut jni_sys::JNIEnv,
    pub clazz: jni_sys::jobject,
    pub internalDataPath: *const ::std::os::raw::c_char,
    pub externalDataPath: *const ::std::os::raw::c_char,
    pub sdkVersion: i32,
    pub instance: *mut ::std::os::raw::c_void,
    pub assetManager: *mut AAssetManager,
    pub obbPath: *const ::std::os::raw::c_char,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ANativeActivityCallbacks {
    pub onStart: ::std::option::Option<unsafe extern "C" fn(activity: *mut ANativeActivity)>,
    pub onResume: ::std::option::Option<unsafe extern "C" fn(activity: *mut ANativeActivity)>,
    pub onSaveInstanceState: ::std::option::Option<
    unsafe extern "C" fn(
        activity: *mut ANativeActivity,
        outSize: *mut usize,
    ) -> *mut ::std::os::raw::c_void,
    >,
    pub onPause: ::std::option::Option<unsafe extern "C" fn(activity: *mut ANativeActivity)>,
    pub onStop: ::std::option::Option<unsafe extern "C" fn(activity: *mut ANativeActivity)>,
    pub onDestroy: ::std::option::Option<unsafe extern "C" fn(activity: *mut ANativeActivity)>,
    pub onWindowFocusChanged: ::std::option::Option<
    unsafe extern "C" fn(activity: *mut ANativeActivity, hasFocus: ::std::os::raw::c_int),
    >,
    pub onNativeWindowCreated: ::std::option::Option<
    unsafe extern "C" fn(activity: *mut ANativeActivity, window: *mut ANativeWindow),
    >,
    pub onNativeWindowResized: ::std::option::Option<
    unsafe extern "C" fn(activity: *mut ANativeActivity, window: *mut ANativeWindow),
    >,
    pub onNativeWindowRedrawNeeded: ::std::option::Option<
    unsafe extern "C" fn(activity: *mut ANativeActivity, window: *mut ANativeWindow),
    >,
    pub onNativeWindowDestroyed: ::std::option::Option<
    unsafe extern "C" fn(activity: *mut ANativeActivity, window: *mut ANativeWindow),
    >,
    pub onInputQueueCreated: ::std::option::Option<
    unsafe extern "C" fn(activity: *mut ANativeActivity, queue: *mut AInputQueue),
    >,
    pub onInputQueueDestroyed: ::std::option::Option<
    unsafe extern "C" fn(activity: *mut ANativeActivity, queue: *mut AInputQueue),
    >,
    pub onContentRectChanged: ::std::option::Option<
    unsafe extern "C" fn(activity: *mut ANativeActivity, rect: *const ARect),
    >,
    pub onConfigurationChanged:
    ::std::option::Option<unsafe extern "C" fn(activity: *mut ANativeActivity)>,
    pub onLowMemory: ::std::option::Option<unsafe extern "C" fn(activity: *mut ANativeActivity)>,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AInputQueue {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ARect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}