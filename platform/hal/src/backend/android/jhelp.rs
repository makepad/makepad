//! Checked JNI helpers shared by the media modules: exception check, a bounded local-reference frame
//! around an Activity call, method lookup, and the worker-thread detach guard.
use super::jni::with_activity;
use makepad_jni_sys::*;
use std::ffi::CStr;

pub(crate) unsafe fn check(env: *mut JNIEnv) -> Result<(), String> {
    if ((**env).ExceptionCheck.unwrap())(env) == 0 {
        return Ok(());
    }
    ((**env).ExceptionDescribe.unwrap())(env);
    ((**env).ExceptionClear.unwrap())(env);
    Err("Java call raised an exception (see logcat)".into())
}

/// A bounded JNI local-reference frame, even on permanently attached native workers.
pub(crate) fn activity<R>(
    f: impl FnOnce(*mut JNIEnv, jobject) -> Result<R, String>,
) -> Result<R, String> {
    with_activity(|env, activity| unsafe {
        if ((**env).PushLocalFrame.unwrap())(env, 16) != 0 {
            check(env)?;
            return Err("cannot allocate JNI local frame".into());
        }
        let result = f(env, activity);
        let exception = check(env);
        ((**env).PopLocalFrame.unwrap())(env, std::ptr::null_mut());
        exception?;
        result
    })
    .ok_or_else(|| "Android Activity is not available".to_string())?
}

/// Retain the VM for the lifetime of a freshly spawned HAL worker. with_activity keeps
/// native threads attached between calls; ART requires them to detach before exiting.
pub(crate) struct WorkerThread(usize);
impl WorkerThread {
    pub(crate) fn new() -> Self {
        Self(
            with_activity(|env, _| unsafe {
                let mut vm = std::ptr::null_mut();
                ((**env).GetJavaVM.unwrap())(env, &mut vm);
                vm as usize
            })
            .unwrap_or(0),
        )
    }
}
impl Drop for WorkerThread {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                let vm = self.0 as *mut JavaVM;
                ((**vm).DetachCurrentThread.unwrap())(vm);
            }
        }
    }
}

pub(crate) unsafe fn method(
    env: *mut JNIEnv,
    object: jobject,
    name: &CStr,
    signature: &CStr,
) -> Result<jmethodID, String> {
    let class = ((**env).GetObjectClass.unwrap())(env, object);
    check(env)?;
    if class.is_null() {
        return Err("missing Java class".into());
    }
    let id = ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
    ((**env).DeleteLocalRef.unwrap())(env, class);
    check(env)?;
    if id.is_null() {
        Err(format!("missing Java method {}", name.to_string_lossy()))
    } else {
        Ok(id)
    }
}
