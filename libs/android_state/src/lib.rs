//! The crate responsible for holding Makepad's Android-specific context states.
//!
//! These two states are:
//! 1. The JavaVM instance initialized by the JNI layer.
//!   * This cannot be set by foreign code outside this crate,
//!     as it is only ever set once during the lifetime of the app process.
//! 2. The current Makepad Activity instance.
//!   * This *can* be set by foreign code outside this crate,
//!     as the underlying Android platform may tear down and reconstruct
//!     the activity instance multiple times during the app's lifetime.
//!   * However, for safety reasons, we only permit a single caller
//!     to obtain the private "set_activity" function, which ensures that
//!     only the internal Makepad framework can set the activity instance.
//!
//! ## Usage
//! You probably want to use the [`robius-android-env`] crate instead of
//! using this crate directly.
//!
//! External users of this crate should only care about two functions:
//! 1. [`get_java_vm()`]: returns a pointer to the JavaVM instance,
//!    through which you can obtain the JNI environment.
//! 2. [`get_activity()`]: returns a pointer to the current Makepad Activity instance.
//!
//! The other functions are intended for Makepad-internal use only,
//! and will not be useful for external users.
//!
//! [`robius-android-env`]: https://github.com/project-robius/robius-android-env

use std::sync::Mutex;
use makepad_jni_sys as jni_sys;

static mut ACTIVITY: jni_sys::jobject = std::ptr::null_mut();
static mut VM: *mut jni_sys::JavaVM = std::ptr::null_mut();

static SET_ACTIVITY_FN: Mutex<Option<unsafe fn(jni_sys::jobject)>> = {
    unsafe fn set_activity(activity: jni_sys::jobject) {
        ACTIVITY = activity;
    }

    std::sync::Mutex::new(Some(set_activity))
};

/// Returns a function that can be used to set the current Makepad Activity instance.
///
/// This will return `Some` only once, which guarantees that only the
/// internal Makepad framework can obtain the function to set the activity instance.
#[doc(hidden)]
pub fn get_activity_setter_fn() -> Option<unsafe fn(jni_sys::jobject)> {
    SET_ACTIVITY_FN.lock().unwrap().take()
}

#[no_mangle]
#[doc(hidden)]
pub unsafe extern "C" fn JNI_OnLoad(
    vm: *mut jni_sys::JavaVM,
    _: std::ffi::c_void,
) -> jni_sys::jint {
    VM = vm as *mut _ as _;

    jni_sys::JNI_VERSION_1_6 as _
}

#[no_mangle]
extern "C" fn jni_on_load(vm: *mut std::ffi::c_void) {
    unsafe {
        VM = vm as _;
    }
}

/// Returns a raw pointer to the JavaVM instance initialized by the JNI layer.
///
/// If the JavaVM instance has not been initialized, this returns a null pointer.
#[inline(always)]
pub fn get_java_vm() -> *mut jni_sys::JavaVM {
    // SAFETY: just returning a raw pointer.
    unsafe { VM }
}

/// Returns a raw pointer to the main Makepad Activity instance.
///
/// Note that the caller should not cache or re-use the returned activity pointer,
/// but should instead re-call this function whenever the activity instance is needed.
/// This is because the activity instance may be destroyed and recreated behind the scenes
/// upon certain system actions, e.g., when the device is rotated,
/// the app is put into split screen, resized/moved, etc.
///
/// If the Activity instance has not been initialized, this returns a null pointer.
#[inline(always)]
pub fn get_activity() -> jni_sys::jobject {
    // SAFETY: just returning a raw pointer.
    unsafe { ACTIVITY }
}
