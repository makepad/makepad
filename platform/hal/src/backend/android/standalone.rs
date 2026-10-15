//! A standalone app APK (`cargo makepad android build -p <app>`): its libmakepad.so is the app
//! itself, natively compiled, with this HAL in the same library. The app's `app_main!` exports
//! JNI_OnLoad and calls [`on_load`], which does what the makepad run host does for a packed app:
//! registers the shell's natives, sends stdout/stderr to logcat, and runs the app on its own
//! thread once the Activity exists (the APK's assets come from it).
use super::jni::with_activity;
use makepad_jni_sys::*;
use std::ffi::{c_char, c_int, c_void, CString};
use std::time::Duration;

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}
#[link(name = "android")]
extern "C" {
    fn AAssetManager_fromJava(env: *mut JNIEnv, assets: jobject) -> *mut c_void;
}
extern "C" {
    fn pipe(fds: *mut c_int) -> c_int;
    fn dup2(old: c_int, new: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut u8, n: usize) -> isize;
    fn close(fd: c_int) -> c_int;
}

const TAG: &std::ffi::CStr = c"makepad";

fn log(text: &str) {
    if let Ok(t) = CString::new(text.replace('\0', " ")) {
        unsafe { __android_log_write(4, TAG.as_ptr(), t.as_ptr()) };
    }
}

/// stdout and stderr go nowhere in an Android app: a pipe on both, each line to logcat.
fn redirect_stdio() {
    unsafe {
        let mut fds = [0 as c_int; 2];
        if pipe(fds.as_mut_ptr()) != 0 {
            return;
        }
        dup2(fds[1], 1);
        dup2(fds[1], 2);
        close(fds[1]);
        let rd = fds[0];
        let _ = std::thread::Builder::new().name("makepad-logcat".into()).spawn(move || {
            let mut buf = vec![0u8; 8192];
            let mut line: Vec<u8> = Vec::new();
            loop {
                let n = read(rd, buf.as_mut_ptr(), buf.len());
                if n <= 0 {
                    break;
                }
                for &b in &buf[..n as usize] {
                    if b == b'\n' {
                        log(&String::from_utf8_lossy(&line));
                        line.clear();
                    } else {
                        line.push(b);
                    }
                }
            }
        });
    }
}

/// The Activity's AssetManager as the HAL's asset source (kept for the life of the process).
unsafe fn set_assets(env: *mut JNIEnv, activity: jobject) -> Result<(), String> {
    let cls = ((**env).GetObjectClass.unwrap())(env, activity);
    let id = ((**env).GetMethodID.unwrap())(env, cls, c"getAssets".as_ptr(), c"()Landroid/content/res/AssetManager;".as_ptr());
    ((**env).DeleteLocalRef.unwrap())(env, cls);
    if id.is_null() {
        return Err("Activity.getAssets not found".into());
    }
    let assets = ((**env).CallObjectMethodA.unwrap())(env, activity, id, std::ptr::null());
    if assets.is_null() {
        return Err("Activity.getAssets returned null".into());
    }
    let global = ((**env).NewGlobalRef.unwrap())(env, assets);
    ((**env).DeleteLocalRef.unwrap())(env, assets);
    super::assets::set_manager(AAssetManager_fromJava(env, global));
    Ok(())
}

/// JNI_OnLoad of a standalone app: registers the natives and starts `app` once the Activity
/// is created; `activity_ready` gets a global reference to it first (the app-side Android state).
/// # Safety
/// Called by ART with its JavaVM, once per process.
pub unsafe fn on_load(vm: *mut c_void, app: fn(), activity_ready: unsafe fn(jobject)) -> jint {
    let vm = vm as *mut JavaVM;
    let mut env: *mut JNIEnv = std::ptr::null_mut();
    if ((**vm).GetEnv.unwrap())(vm, &mut env as *mut _ as _, JNI_VERSION_1_6) != 0 {
        return JNI_ERR;
    }
    redirect_stdio();
    std::panic::set_hook(Box::new(|info| {
        log(&format!("panic: {info}"));
        log(&format!("{}", std::backtrace::Backtrace::force_capture()));
    }));
    let class = ((**env).FindClass.unwrap())(env, c"dev/makepad/android/MakepadNative".as_ptr());
    if class.is_null() {
        log("no dev.makepad.android.MakepadNative class");
        return JNI_ERR;
    }
    let result = super::register_natives(env, class);
    ((**env).DeleteLocalRef.unwrap())(env, class);
    match result {
        Ok(pending) if pending.is_empty() => {}
        Ok(pending) => log(&format!("natives no HAL module owns: {}", pending.join(", "))),
        Err(error) => {
            log(&format!("HAL registration failed: {error}"));
            return JNI_ERR;
        }
    }
    let spawned = std::thread::Builder::new().name("makepad-app".into()).stack_size(16 << 20).spawn(move || {
        // The Activity's onCreate registers it right after loading this library.
        loop {
            match with_activity(|env, activity| unsafe {
                activity_ready(((**env).NewGlobalRef.unwrap())(env, activity));
                set_assets(env, activity)
            }) {
                Some(Ok(())) => break,
                Some(Err(error)) => {
                    log(&format!("APK assets: {error}"));
                    break;
                }
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        }
        app();
        // The app ended (its Activity finished: Back on the last screen). A later
        // launch may reuse this process with a new Activity, and nothing would run
        // it: end the process, so the next launch starts the app again.
        log("the app returned; ending the process");
        std::process::exit(0);
    });
    if let Err(error) = spawned {
        log(&format!("cannot start the app thread: {error}"));
        return JNI_ERR;
    }
    JNI_VERSION_1_6
}
