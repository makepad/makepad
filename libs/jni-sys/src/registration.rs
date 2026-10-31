//! Shared RegisterNatives transport for host-native and JIT exports. Java reflection is the
//! authority for descriptors; function resolvers choose which module owns each native.
use crate::*;
use std::ffi::{c_void, CStr, CString};

/// JNI short-name escaping (JNI specification, Resolving Native Method Names).
pub fn mangle(name: &str) -> String {
    let mut out = String::new();
    for c in name.encode_utf16() {
        match c {
            48..=57 | 65..=90 | 97..=122 => out.push(char::from_u32(c as u32).unwrap()),
            95 => out.push_str("_1"), 59 => out.push_str("_2"), 91 => out.push_str("_3"),
            46 | 47 => out.push('_'),
            c => out.push_str(&format!("_0{c:04x}")),
        }
    }
    out
}

/// # Safety
/// `env` is this thread's JNI environment, `class` is live, and every resolved address has
/// exactly the reflected JNI signature. Missing methods remain available to other modules.
pub unsafe fn register(
    env: *mut JNIEnv, class: jclass, natives: &str,
    mut resolve: impl FnMut(&str, &str) -> Result<Option<*mut c_void>, String>,
) -> Result<Vec<String>, String> {
    let mut strings = Vec::new();
    let mut addresses = Vec::new();
    let mut missing = Vec::new();
    for line in natives.lines().filter(|l| !l.is_empty()) {
        let (name, sig) = line.split_once(':').ok_or_else(|| format!("bad native method line {line:?}"))?;
        match resolve(name, sig)? {
            Some(addr) => {
                strings.push((CString::new(name).map_err(|e| e.to_string())?, CString::new(sig).map_err(|e| e.to_string())?));
                addresses.push(addr);
            }
            None => missing.push(name.to_string()),
        }
    }
    let methods: Vec<_> = strings.iter().zip(addresses).map(|((name, sig), addr)| JNINativeMethod {
        name: name.as_ptr() as _, signature: sig.as_ptr() as _, fnPtr: addr,
    }).collect();
    if !methods.is_empty() && ((**env).RegisterNatives.unwrap())(env, class, methods.as_ptr(), methods.len() as jint) != 0 {
        return Err("RegisterNatives failed".into());
    }
    Ok(missing)
}

pub unsafe fn string(env: *mut JNIEnv, s: jstring) -> String {
    if s.is_null() { return String::new(); }
    let len = ((**env).GetStringLength.unwrap())(env, s);
    let p = ((**env).GetStringChars.unwrap())(env, s, std::ptr::null_mut());
    if p.is_null() { return String::new(); }
    let out = String::from_utf16_lossy(std::slice::from_raw_parts(p, len as usize));
    ((**env).ReleaseStringChars.unwrap())(env, s, p);
    out
}

pub unsafe fn object(env: *mut JNIEnv, obj: jobject, name: &CStr, sig: &CStr, args: &[jvalue]) -> jobject {
    let cls = ((**env).GetObjectClass.unwrap())(env, obj);
    let id = ((**env).GetMethodID.unwrap())(env, cls, name.as_ptr(), sig.as_ptr());
    ((**env).DeleteLocalRef.unwrap())(env, cls);
    if id.is_null() { return std::ptr::null_mut(); }
    ((**env).CallObjectMethodA.unwrap())(env, obj, id, args.as_ptr())
}

unsafe fn descriptor(env: *mut JNIEnv, cls: jobject) -> String {
    let s = object(env, cls, c"getName", c"()Ljava/lang/String;", &[]);
    let name = string(env, s);
    ((**env).DeleteLocalRef.unwrap())(env, s);
    match name.as_str() {
        "void" => "V".into(), "boolean" => "Z".into(), "byte" => "B".into(),
        "char" => "C".into(), "short" => "S".into(), "int" => "I".into(),
        "long" => "J".into(), "float" => "F".into(), "double" => "D".into(),
        n if n.starts_with('[') => n.replace('.', "/"),
        n => format!("L{};", n.replace('.', "/")),
    }
}

/// The same `name:descriptor` list RunActivity supplies to the JIT host, for hosts that
/// register directly from JNI_OnLoad without a RunActivity boot.
/// # Safety
/// `env` belongs to the calling thread and `class` is a live java.lang.Class reference.
pub unsafe fn reflect(env: *mut JNIEnv, class: jclass) -> Result<String, String> {
    if ((**env).PushLocalFrame.unwrap())(env, 32) != 0 { return Err("JNI local frame allocation failed".into()); }
    let result = (|| {
        let methods = object(env, class, c"getDeclaredMethods", c"()[Ljava/lang/reflect/Method;", &[]);
        if methods.is_null() { return Err("getDeclaredMethods failed".into()); }
        let mut out = String::new();
        for i in 0..((**env).GetArrayLength.unwrap())(env, methods) {
            if ((**env).PushLocalFrame.unwrap())(env, 16) != 0 { return Err("JNI local frame allocation failed".into()); }
            let method = ((**env).GetObjectArrayElement.unwrap())(env, methods, i);
            let cls = ((**env).GetObjectClass.unwrap())(env, method);
            let mid = ((**env).GetMethodID.unwrap())(env, cls, c"getModifiers".as_ptr(), c"()I".as_ptr());
            if !mid.is_null() && ((**env).CallIntMethodA.unwrap())(env, method, mid, std::ptr::null()) & 0x100 != 0 {
                let name = object(env, method, c"getName", c"()Ljava/lang/String;", &[]);
                out.push_str(&string(env, name)); out.push_str(":(");
                let params = object(env, method, c"getParameterTypes", c"()[Ljava/lang/Class;", &[]);
                if !params.is_null() {
                    for j in 0..((**env).GetArrayLength.unwrap())(env, params) {
                        let ty = ((**env).GetObjectArrayElement.unwrap())(env, params, j);
                        out.push_str(&descriptor(env, ty));
                        ((**env).DeleteLocalRef.unwrap())(env, ty);
                    }
                }
                out.push(')');
                let ret = object(env, method, c"getReturnType", c"()Ljava/lang/Class;", &[]);
                if !ret.is_null() { out.push_str(&descriptor(env, ret)); }
                out.push('\n');
            }
            ((**env).PopLocalFrame.unwrap())(env, std::ptr::null_mut());
            if ((**env).ExceptionCheck.unwrap())(env) != 0 { return Err("native reflection failed".into()); }
        }
        Ok(out)
    })();
    ((**env).PopLocalFrame.unwrap())(env, std::ptr::null_mut());
    result
}
