//! Java-side TLS socket streams for the process (the Activity's `openSocketStream` family). Android has
//! no libssl for native code; the network crate's socket factory (makepad-remote's tunnel, HTTPS) is
//! backed by these calls. The host registers the factory; any thread may call.
use super::jni::with_activity;
use makepad_jni_sys::*;
use std::ffi::CString;

fn call<R>(f: impl FnOnce(*mut JNIEnv, jobject) -> R) -> Option<R> { with_activity(f) }

/// Opens a stream (TLS when `tls`); `id` names it for the other calls. False when Java refused.
pub fn open(id: i64, host: &str, port: i32, tls: bool, ignore_cert: bool) -> bool {
    let Ok(host) = CString::new(host) else { return false };
    call(|env, act| unsafe {
        let s = ((**env).NewStringUTF.unwrap())(env, host.as_ptr());
        let m = method(env, act, c"openSocketStream", c"(JLjava/lang/String;IZZ)Z");
        let ok = !m.is_null() && ((**env).CallBooleanMethodA.unwrap())(env, act, m, [jvalue { j: id }, jvalue { l: s }, jvalue { i: port }, jvalue { z: tls as u8 }, jvalue { z: ignore_cert as u8 }].as_ptr()) != 0;
        ((**env).DeleteLocalRef.unwrap())(env, s);
        ok && ((**env).ExceptionCheck.unwrap())(env) == 0
    }).unwrap_or(false)
}

/// Reads into `out` (blocking until some bytes arrive or the read timeout passes). `Some(n)`: `n`
/// bytes, 0 for a read timeout; `None`: the stream is closed or failed. Java's reader thread
/// fills a ring; this takes what it holds straight into `out` (a direct buffer over it: no array
/// per read, one copy).
pub fn read_into(id: i64, out: &mut [u8]) -> Option<usize> {
    static READ: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    call(|env, act| unsafe {
        let m = cached(env, act, &READ, c"socketStreamReadInto", c"(JLjava/nio/ByteBuffer;)I");
        if m.is_null() { return None; }
        let buf = ((**env).NewDirectByteBuffer.unwrap())(env, out.as_mut_ptr() as *mut std::ffi::c_void, out.len() as i64);
        if buf.is_null() { clear(env); return None; }
        let n = ((**env).CallIntMethodA.unwrap())(env, act, m, [jvalue { j: id }, jvalue { l: buf }].as_ptr());
        ((**env).DeleteLocalRef.unwrap())(env, buf);
        if ((**env).ExceptionCheck.unwrap())(env) != 0 { clear(env); return None; }
        if n < 0 { None } else { Some((n as usize).min(out.len())) }
    }).flatten()
}

/// Bytes written, or -1.
pub fn write(id: i64, data: &[u8]) -> i32 {
    call(|env, act| unsafe {
        static WRITE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let m = cached(env, act, &WRITE, c"socketStreamWrite", c"(J[B)I");
        if m.is_null() { return -1; }
        let arr = ((**env).NewByteArray.unwrap())(env, data.len() as i32);
        ((**env).SetByteArrayRegion.unwrap())(env, arr, 0, data.len() as i32, data.as_ptr() as *const jbyte);
        let n = ((**env).CallIntMethodA.unwrap())(env, act, m, [jvalue { j: id }, jvalue { l: arr }].as_ptr());
        ((**env).DeleteLocalRef.unwrap())(env, arr);
        if ((**env).ExceptionCheck.unwrap())(env) != 0 { clear(env); return -1; }
        n
    }).unwrap_or(-1)
}

pub fn set_timeout(id: i64, read: bool, ms: i32) {
    call(|env, act| unsafe {
        let m = method(env, act, if read { c"socketStreamSetReadTimeout" } else { c"socketStreamSetWriteTimeout" }, c"(JI)V");
        if !m.is_null() { ((**env).CallVoidMethodA.unwrap())(env, act, m, [jvalue { j: id }, jvalue { i: ms }].as_ptr()); }
    });
}

/// DER of the server's leaf certificate, once handshaken.
pub fn peer_certificate(id: i64) -> Option<Vec<u8>> {
    call(|env, act| unsafe {
        let m = method(env, act, c"socketStreamPeerCertificate", c"(J)[B");
        if m.is_null() { return None; }
        let arr = ((**env).CallObjectMethodA.unwrap())(env, act, m, [jvalue { j: id }].as_ptr());
        if arr.is_null() { clear(env); return None; }
        let len = ((**env).GetArrayLength.unwrap())(env, arr) as usize;
        let mut out = vec![0u8; len];
        ((**env).GetByteArrayRegion.unwrap())(env, arr, 0, len as i32, out.as_mut_ptr() as *mut jbyte);
        ((**env).DeleteLocalRef.unwrap())(env, arr);
        Some(out)
    }).flatten()
}

pub fn close(id: i64) {
    call(|env, act| unsafe {
        let m = method(env, act, c"closeSocketStream", c"(J)V");
        if !m.is_null() { ((**env).CallVoidMethodA.unwrap())(env, act, m, [jvalue { j: id }].as_ptr()); }
        clear(env);
    });
}

unsafe fn clear(env: *mut JNIEnv) {
    if ((**env).ExceptionCheck.unwrap())(env) != 0 { ((**env).ExceptionClear.unwrap())(env); }
}
/// A method id looked up once (the Activity's class lives as long as the process).
unsafe fn cached(env: *mut JNIEnv, obj: jobject, slot: &std::sync::OnceLock<usize>, name: &std::ffi::CStr, sig: &std::ffi::CStr) -> jmethodID {
    if let Some(m) = slot.get() {
        return *m as jmethodID;
    }
    let m = method(env, obj, name, sig);
    if !m.is_null() {
        let _ = slot.set(m as usize);
    }
    m
}
unsafe fn method(env: *mut JNIEnv, obj: jobject, name: &std::ffi::CStr, sig: &std::ffi::CStr) -> jmethodID {
    let cls = ((**env).GetObjectClass.unwrap())(env, obj);
    let id = ((**env).GetMethodID.unwrap())(env, cls, name.as_ptr(), sig.as_ptr());
    ((**env).DeleteLocalRef.unwrap())(env, cls);
    id
}
