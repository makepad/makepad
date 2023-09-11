//! Little helpers to writing JNI code.
//! Aimed to reduce amount of (**env).Function.unwrap() calls in the code.
//! This belongs to a separate crate!

#[macro_export]
/// Find an <init> method with given signature
/// on $obj class
/// and call a NewObject jni function with given extra arguments
macro_rules! new_object {
    ($env:expr, $class:expr, $sig:expr $(, $args:expr)*) => {{
        let find_class = (**$env).FindClass.unwrap();
        let get_method_id = (**$env).GetMethodID.unwrap();
        let new_object = (**$env).NewObject.unwrap();

        let class = std::ffi::CString::new($class).unwrap();
        let sig = std::ffi::CString::new($sig).unwrap();
        let class = find_class($env, class.as_ptr() as _);

        let constructor = get_method_id($env, class, b"<init>\0".as_ptr() as _, sig.as_ptr() as _);

        new_object($env, class, constructor, $($args,)*)
    }};
}

#[macro_export]
macro_rules! call_method {
    ($fn:tt, $env:expr, $obj:expr, $method:expr, $sig:expr $(, $args:expr)*) => {{
        let get_object_class = (**$env).GetObjectClass.unwrap();
        let get_method_id = (**$env).GetMethodID.unwrap();
        let call_object_method = (**$env).$fn.unwrap();

        let method = std::ffi::CString::new($method).unwrap();
        let sig = std::ffi::CString::new($sig).unwrap();
        let class = get_object_class($env, $obj);

        assert!(!class.is_null());

        let method = get_method_id($env, class, method.as_ptr() as _, sig.as_ptr() as _);
        assert!(!method.is_null());

        call_object_method($env, $obj, method, $($args,)*)
    }};
}

#[macro_export]
macro_rules! call_object_method {
    ($env:expr, $obj:expr, $method:expr, $sig:expr $(, $args:expr)*) => {{
        $crate::call_method!(CallObjectMethod, $env, $obj, $method, $sig $(, $args)*)
    }};
}

#[macro_export]
macro_rules! call_int_method {
    ($env:expr, $obj:expr, $method:expr, $sig:expr $(, $args:expr)*) => {{
        $crate::call_method!(CallIntMethod, $env, $obj, $method, $sig $(, $args)*)
    }};
}

#[macro_export]
macro_rules! call_void_method {
    ($env:expr, $obj:expr, $method:expr, $sig:expr $(, $args:expr)*) => {{
        $crate::call_method!(CallVoidMethod, $env, $obj, $method, $sig $(, $args)*)
    }};
}

#[macro_export]
macro_rules! call_bool_method {
    ($env:expr, $obj:expr, $method:expr, $sig:expr $(, $args:expr)*) => {{
        $crate::call_method!(CallBooleanMethod, $env, $obj, $method, $sig $(, $args)*)
    }};
}

#[macro_export]
macro_rules! get_utf_str {
    ($env:expr, $obj:expr) => {{
        let string = (**$env).GetStringUTFChars.unwrap()($env, $obj, std::ptr::null_mut());
        let string = std::ffi::CStr::from_ptr(string);
        string.to_str().unwrap()
    }};
}

#[macro_export]
macro_rules! new_global_ref {
    ($env:expr, $obj:expr) => {{
        (**$env).NewGlobalRef.unwrap()($env, $obj)
    }};
}

#[macro_export]
macro_rules! new_local_ref {
    ($env:expr, $obj:expr) => {{
        (**$env).NewLocalRef.unwrap()($env, $obj)
    }};
}

pub use {
    call_bool_method, call_int_method, call_method, call_object_method, call_void_method,
    get_utf_str, new_global_ref, new_local_ref, new_object,
};
