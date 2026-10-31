//! Fleet identity crypto and device stats for the process (host and program alike: the `hal_fleet_*`
//! and `hal_battery` ops in lib.rs). Native code on Android has no libcrypto/libssl, so P-256
//! (keys, ECDSA, ECDH) and the TLS server are the platform's Java security providers'
//! (FleetTls.java: java.security, an SSLEngine per session), the battery is BatteryManager's sticky
//! broadcast (FleetTls.battery). The class is looked up once while JNI_OnLoad registers the natives
//! (the app's class loader); any thread calls. Keys are X9.63 (04 || X || Y || d, 97 bytes).

use makepad_jni_sys as sys;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// global ref of dev/makepad/android/FleetTls
static CLASS: AtomicUsize = AtomicUsize::new(0);

/// While the natives are registered (JNI_OnLoad's thread: the app's class loader).
pub(crate) unsafe fn init(env: *mut sys::JNIEnv) -> Result<(), String> {
    let class = ((**env).FindClass.unwrap())(env, c"dev/makepad/android/FleetTls".as_ptr());
    if class.is_null() {
        clear(env);
        return Err("the APK has no dev/makepad/android/FleetTls".into());
    }
    let global = ((**env).NewGlobalRef.unwrap())(env, class);
    ((**env).DeleteLocalRef.unwrap())(env, class);
    CLASS.store(global as usize, Ordering::Release);
    Ok(())
}

unsafe fn clear(env: *mut sys::JNIEnv) -> Option<String> {
    if ((**env).ExceptionCheck.unwrap())(env) == 0 {
        return None;
    }
    ((**env).ExceptionDescribe.unwrap())(env);
    ((**env).ExceptionClear.unwrap())(env);
    Some("a Java security call failed (see logcat)".into())
}

/// Runs `f` in a local frame with a JNI environment (the HAL's [`super::jni::on_java`]: never
/// attaches the caller, whose stack may be the JIT's).
fn with_env<R>(f: impl FnOnce(*mut sys::JNIEnv, sys::jclass) -> Result<R, String>) -> Result<R, String> {
    let class = CLASS.load(Ordering::Acquire) as sys::jclass;
    if class.is_null() {
        return Err("FleetTls is not initialised".into());
    }
    super::jni::on_java(|env| unsafe {
        if ((**env).PushLocalFrame.unwrap())(env, 32) != 0 {
            clear(env);
            return Err("no JNI local frame".into());
        }
        let r = f(env, class);
        let ex = clear(env);
        ((**env).PopLocalFrame.unwrap())(env, std::ptr::null_mut());
        match ex {
            Some(e) => Err(e),
            None => r,
        }
    })
    .unwrap_or_else(|| Err("no Java VM".into()))
}

unsafe fn bytes_in(env: *mut sys::JNIEnv, b: &[u8]) -> sys::jbyteArray {
    let arr = ((**env).NewByteArray.unwrap())(env, b.len() as i32);
    ((**env).SetByteArrayRegion.unwrap())(env, arr, 0, b.len() as i32, b.as_ptr() as *const sys::jbyte);
    arr
}

unsafe fn bytes_out(env: *mut sys::JNIEnv, arr: sys::jobject) -> Result<Vec<u8>, String> {
    if arr.is_null() {
        return Err(clear(env).unwrap_or_else(|| "a Java security call returned nothing".into()));
    }
    let len = ((**env).GetArrayLength.unwrap())(env, arr) as usize;
    let mut out = vec![0u8; len];
    ((**env).GetByteArrayRegion.unwrap())(env, arr, 0, len as i32, out.as_mut_ptr() as *mut sys::jbyte);
    Ok(out)
}

unsafe fn static_obj(env: *mut sys::JNIEnv, class: sys::jclass, name: &std::ffi::CStr, sig: &std::ffi::CStr, args: &[sys::jvalue]) -> Result<sys::jobject, String> {
    let m = ((**env).GetStaticMethodID.unwrap())(env, class, name.as_ptr(), sig.as_ptr());
    if m.is_null() {
        clear(env);
        return Err(format!("FleetTls.{} is missing", name.to_string_lossy()));
    }
    let r = ((**env).CallStaticObjectMethodA.unwrap())(env, class, m, args.as_ptr());
    match clear(env) {
        Some(e) => Err(e),
        None => Ok(r),
    }
}

unsafe fn method(env: *mut sys::JNIEnv, obj: sys::jobject, name: &std::ffi::CStr, sig: &std::ffi::CStr) -> Result<sys::jmethodID, String> {
    let cls = ((**env).GetObjectClass.unwrap())(env, obj);
    let m = ((**env).GetMethodID.unwrap())(env, cls, name.as_ptr(), sig.as_ptr());
    ((**env).DeleteLocalRef.unwrap())(env, cls);
    if m.is_null() {
        clear(env);
        return Err(format!("FleetTls#{} is missing", name.to_string_lossy()));
    }
    Ok(m)
}

unsafe fn call_bytes(env: *mut sys::JNIEnv, obj: sys::jobject, name: &std::ffi::CStr, sig: &std::ffi::CStr, args: &[sys::jvalue]) -> Result<Vec<u8>, String> {
    let m = method(env, obj, name, sig)?;
    let r = ((**env).CallObjectMethodA.unwrap())(env, obj, m, args.as_ptr());
    if let Some(e) = clear(env) {
        return Err(e);
    }
    bytes_out(env, r)
}

unsafe fn call_void(env: *mut sys::JNIEnv, obj: sys::jobject, name: &std::ffi::CStr, sig: &std::ffi::CStr, args: &[sys::jvalue]) -> Result<(), String> {
    let m = method(env, obj, name, sig)?;
    ((**env).CallVoidMethodA.unwrap())(env, obj, m, args.as_ptr());
    clear(env).map_or(Ok(()), Err)
}

unsafe fn call_bool(env: *mut sys::JNIEnv, obj: sys::jobject, name: &std::ffi::CStr) -> Result<bool, String> {
    let m = method(env, obj, name, c"()Z")?;
    let r = ((**env).CallBooleanMethodA.unwrap())(env, obj, m, std::ptr::null());
    clear(env).map_or(Ok(r != 0), Err)
}

pub fn generate_p256() -> Result<Vec<u8>, String> {
    with_env(|env, class| unsafe { bytes_out(env, static_obj(env, class, c"generateP256", c"()[B", &[])?) })
}

pub fn sign(key: &[u8], msg: &[u8]) -> Result<Vec<u8>, String> {
    with_env(|env, class| unsafe {
        let (k, m) = (bytes_in(env, key), bytes_in(env, msg));
        bytes_out(env, static_obj(env, class, c"sign", c"([B[B)[B", &[sys::jvalue { l: k }, sys::jvalue { l: m }])?)
    })
}

pub fn ecdh(key: &[u8], point: &[u8]) -> Result<Vec<u8>, String> {
    with_env(|env, class| unsafe {
        let (k, p) = (bytes_in(env, key), bytes_in(env, point));
        bytes_out(env, static_obj(env, class, c"ecdh", c"([B[B)[B", &[sys::jvalue { l: k }, sys::jvalue { l: p }])?)
    })
}

pub fn verify(point: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    with_env(|env, class| unsafe {
        let (p, m, s) = (bytes_in(env, point), bytes_in(env, msg), bytes_in(env, sig));
        let id = ((**env).GetStaticMethodID.unwrap())(env, class, c"verify".as_ptr(), c"([B[B[B)Z".as_ptr());
        if id.is_null() {
            clear(env);
            return Ok(false);
        }
        let ok = ((**env).CallStaticBooleanMethodA.unwrap())(env, class, id, [sys::jvalue { l: p }, sys::jvalue { l: m }, sys::jvalue { l: s }].as_ptr());
        Ok(clear(env).is_none() && ok != 0)
    })
    .unwrap_or(false)
}

/// Battery level 0..=100 and whether it charges (BatteryManager's sticky broadcast), or None.
pub fn battery() -> Option<(u32, bool)> {
    let (_, activity) = super::jni::vm_and_activity()?;
    let activity = activity as usize;
    with_env(|env, class| unsafe {
        let id = ((**env).GetStaticMethodID.unwrap())(env, class, c"battery".as_ptr(), c"(Landroid/content/Context;)I".as_ptr());
        if id.is_null() {
            clear(env);
            return Ok(None);
        }
        let v = ((**env).CallStaticIntMethodA.unwrap())(env, class, id, [sys::jvalue { l: activity as sys::jobject }].as_ptr());
        if clear(env).is_some() || v < 0 {
            return Ok(None);
        }
        Ok(Some(((v & 0xff) as u32, v & 0x100 != 0)))
    })
    .ok()
    .flatten()
}

// ---- TLS server sessions: handles name Java objects (global references) the table keeps, with
// the bytes a session produced and the caller has not taken yet

struct Entry {
    obj: usize,
    app: Vec<u8>,
    net: Vec<u8>,
}

fn table() -> &'static Mutex<HashMap<i64, Entry>> {
    static T: OnceLock<Mutex<HashMap<i64, Entry>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}
static NEXT: AtomicI64 = AtomicI64::new(1);

fn keep(obj: usize) -> i64 {
    let h = NEXT.fetch_add(1, Ordering::Relaxed);
    table().lock().unwrap().insert(h, Entry { obj, app: Vec::new(), net: Vec::new() });
    h
}
fn get(h: i64) -> Result<usize, String> {
    table().lock().unwrap().get(&h).map(|e| e.obj).ok_or_else(|| format!("no TLS handle {h}"))
}

/// Frees a server or session handle.
pub fn free(h: i64) {
    if let Some(e) = table().lock().unwrap().remove(&h) {
        let _ = with_env(|env, _| unsafe {
            ((**env).DeleteGlobalRef.unwrap())(env, e.obj as sys::jobject);
            Ok(())
        });
    }
}

/// A TLS server context for this certificate (DER) and key.
pub fn server(cert_der: &[u8], key: &[u8]) -> Result<i64, String> {
    let g = with_env(|env, class| unsafe {
        let (c, k) = (bytes_in(env, cert_der), bytes_in(env, key));
        let ctx = static_obj(env, class, c"serverContext", c"([B[B)Ljava/lang/Object;", &[sys::jvalue { l: c }, sys::jvalue { l: k }])?;
        Ok(((**env).NewGlobalRef.unwrap())(env, ctx) as usize)
    })?;
    Ok(keep(g))
}

/// A new server session of context `server` (before its handshake).
pub fn session(server: i64) -> Result<i64, String> {
    let ctx = get(server)?;
    let g = with_env(|env, class| unsafe {
        let s = static_obj(env, class, c"session", c"(Ljava/lang/Object;)Ldev/makepad/android/FleetTls;", &[sys::jvalue { l: ctx as sys::jobject }])?;
        Ok(((**env).NewGlobalRef.unwrap())(env, s) as usize)
    })?;
    Ok(keep(g))
}

fn stash(h: i64, app: Vec<u8>, net: Vec<u8>) {
    if let Some(e) = table().lock().unwrap().get_mut(&h) {
        e.app.extend_from_slice(&app);
        e.net.extend_from_slice(&net);
    }
}

/// Feeds ciphertext (empty: just advance the handshake); what it produced waits for [`take`].
/// Returns (handshaking, closed).
pub fn feed(h: i64, net_in: &[u8]) -> Result<(bool, bool), String> {
    let s = get(h)?;
    let (app, net, hs, closed) = with_env(|env, _| unsafe {
        let s = s as sys::jobject;
        if net_in.is_empty() {
            call_void(env, s, c"pump", c"()V", &[])?;
        } else {
            call_void(env, s, c"feed", c"([B)V", &[sys::jvalue { l: bytes_in(env, net_in) }])?;
        }
        Ok((call_bytes(env, s, c"takeApp", c"()[B", &[])?, call_bytes(env, s, c"takeNet", c"()[B", &[])?, call_bool(env, s, c"handshaking")?, call_bool(env, s, c"closed")?))
    })?;
    stash(h, app, net);
    Ok((hs, closed))
}

/// Encrypts plaintext; the ciphertext waits for [`take`].
pub fn wrap(h: i64, app: &[u8]) -> Result<(), String> {
    let s = get(h)?;
    let net = with_env(|env, _| unsafe { call_bytes(env, s as sys::jobject, c"wrap", c"([B)[B", &[sys::jvalue { l: bytes_in(env, app) }]) })?;
    stash(h, Vec::new(), net);
    Ok(())
}

/// Stages the close_notify; it waits for [`take`].
pub fn close(h: i64) {
    if let Ok(s) = get(h) {
        let net = with_env(|env, _| unsafe { call_bytes(env, s as sys::jobject, c"close", c"()[B", &[]) }).unwrap_or_default();
        stash(h, Vec::new(), net);
    }
}

/// Moves up to `out.len()` waiting bytes (plaintext when `app`, else ciphertext to send) into
/// `out`; returns how many (0: none waiting), -1 for an unknown handle.
pub fn take(h: i64, app: bool, out: &mut [u8]) -> i64 {
    let mut t = table().lock().unwrap();
    let Some(e) = t.get_mut(&h) else { return -1 };
    let buf = if app { &mut e.app } else { &mut e.net };
    let n = buf.len().min(out.len());
    out[..n].copy_from_slice(&buf[..n]);
    buf.drain(..n);
    n as i64
}
