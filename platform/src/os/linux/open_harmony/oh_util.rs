#[allow(dead_code)]
use napi_ohos::sys::*;
use std::ffi::CString;
use std::ptr::null_mut;

fn value_type_to_string(val_type: &napi_valuetype) -> String {
    match *val_type {
        ValueType::napi_undefined => "undefined".to_string(),
        ValueType::napi_null => "null".to_string(),
        ValueType::napi_boolean => "boolean".to_string(),
        ValueType::napi_number => "number".to_string(),
        ValueType::napi_string => "string".to_string(),
        ValueType::napi_symbol => "symbol".to_string(),
        ValueType::napi_object => "object".to_string(),
        ValueType::napi_function => "function".to_string(),
        ValueType::napi_external => "external".to_string(),
        _ => "undefined".to_string(),
    }
}

pub fn get_value_string(raw_env: napi_env, str_value: napi_value) -> Option<String> {
    let mut len = 0;
    let napi_status =
        unsafe { napi_get_value_string_utf8(raw_env, str_value, null_mut(), 0, &mut len) };
    if napi_status != Status::napi_ok {
        crate::error!("failed to get string from napi_value");
        return None;
    }

    len += 1;
    let mut ret = Vec::with_capacity(len);
    let buf_ptr = ret.as_mut_ptr();
    let mut written_char_count = 0;
    let napi_status = unsafe {
        napi_get_value_string_utf8(raw_env, str_value, buf_ptr, len, &mut written_char_count)
    };
    if napi_status != Status::napi_ok {
        crate::error!("failed to get string from napi_value");
        return None;
    }

    let mut ret = std::mem::ManuallyDrop::new(ret);
    let buf_ptr = ret.as_mut_ptr();
    let bytes = unsafe { Vec::from_raw_parts(buf_ptr as *mut u8, written_char_count, len) };
    match String::from_utf8(bytes) {
        Err(e) => {
            crate::error!("failed to read utf8 string, {}", e);
            return None;
        }
        Ok(s) => Some(s),
    }
}

pub fn get_value_f64(raw_env: napi_env, f64_value: napi_value) -> Option<f64> {
    let mut result: f64 = 0.0;
    let napi_status = unsafe { napi_get_value_double(raw_env, f64_value, &mut result) };
    if napi_status != Status::napi_ok {
        crate::error!("failed to get f64 value from napi_value");
        return None;
    }
    return Some(result);
}

pub fn get_uv_loop(raw_env: napi_env) -> Option<*mut uv_loop_s> {
    let mut uv_loop = std::ptr::null_mut();
    let napi_status = unsafe { napi_get_uv_event_loop(raw_env, &mut uv_loop) };
    if napi_status != Status::napi_ok {
        crate::error!("failed to get uv loop from env");
        return None;
    }
    return Some(uv_loop);
}

pub fn get_object_property(
    raw_env: napi_env,
    object_value: napi_value,
    property_name: &str,
) -> Option<napi_value> {
    let cname = CString::new(property_name).ok()?;
    let mut result = null_mut();
    let napi_status =
        unsafe { napi_get_named_property(raw_env, object_value, cname.as_ptr(), &mut result) };
    if napi_status != Status::napi_ok {
        crate::error!("get property {} failed", property_name);
        return None;
    }
    let mut napi_type: napi_valuetype = 0;
    let _ = unsafe { napi_typeof(raw_env, result, &mut napi_type) };
    if napi_type == ValueType::napi_undefined {
        crate::error!("property {} is undefined", property_name);
        return None;
    }
    return Some(result);
}

pub fn get_global_this(raw_env: napi_env) -> Option<napi_value> {
    let mut global_obj = std::ptr::null_mut();
    let napi_status = unsafe { napi_get_global(raw_env, &mut global_obj) };
    if napi_status != Status::napi_ok {
        crate::error!("get global from env failed, error code = {}", napi_status);
        return None;
    }
    crate::log!("get global from env success");

    let mut global_this = std::ptr::null_mut();
    let napi_status = unsafe {
        napi_get_named_property(
            raw_env,
            global_obj,
            c"globalThis".as_ptr(),
            &mut global_this,
        )
    };
    if napi_status != Status::napi_ok {
        crate::error!(
            "get globalThis from global failed, error code = {}",
            napi_status
        );
        return None;
    }
    let mut napi_type: napi_valuetype = 0;
    let _ = unsafe { napi_typeof(raw_env, global_this, &mut napi_type) };
    if napi_type != ValueType::napi_object {
        crate::error!(
            "globalThis expect to be object, current data type = {}",
            value_type_to_string(&napi_type)
        );
        return None;
    }
    crate::log!("get globalThis from global success");
    return Some(global_this);
}

pub fn get_global_context(raw_env: napi_env) -> Option<napi_value> {
    let global_this = get_global_this(raw_env);
    if global_this.is_none() {
        return None;
    }

    let mut get_context_fn = std::ptr::null_mut();
    let napi_status = unsafe {
        napi_get_named_property(
            raw_env,
            global_this?,
            c"getContext".as_ptr(),
            &mut get_context_fn,
        )
    };
    if napi_status != Status::napi_ok {
        crate::error!(
            "get getContext from globalThis failed, error code = {}",
            napi_status
        );
        return None;
    }
    let mut napi_type: napi_valuetype = 0;
    let _ = unsafe { napi_typeof(raw_env, get_context_fn, &mut napi_type) };
    if napi_type != ValueType::napi_function {
        crate::error!(
            "getContext expect to be function, current data type = {}",
            value_type_to_string(&napi_type)
        );
        return None;
    }
    crate::log!("get getContext function success");

    let mut ctx_recv = std::ptr::null_mut();
    unsafe {
        let _ = napi_get_undefined(raw_env, &mut ctx_recv);
    }
    let mut get_context_result = std::ptr::null_mut();
    let napi_status = unsafe {
        napi_call_function(
            raw_env,
            ctx_recv,
            get_context_fn,
            0,
            std::ptr::null(),
            &mut get_context_result,
        )
    };
    if napi_status != Status::napi_ok {
        crate::error!("call getContext() failed, error code = {}", napi_status);
        return None;
    }
    napi_type = 0;
    let _ = unsafe { napi_typeof(raw_env, get_context_result, &mut napi_type) };
    if napi_type != ValueType::napi_object {
        crate::error!(
            "getContext() result expect to be object, current data type = {}",
            value_type_to_string(&napi_type)
        );
        return None;
    }
    crate::log!("call getContext() succcess");
    return Some(get_context_result);
}

pub fn get_files_dir(raw_env: napi_env) -> Option<String> {
    let ctx = get_global_context(raw_env);
    if ctx.is_none() {
        return None;
    }
    let file_dirs = get_object_property(raw_env, ctx?, "filesDir");
    if file_dirs.is_none() {
        crate::error!("failed to get filesDir from global context");
        return None;
    }
    let str_val = get_value_string(raw_env, file_dirs?);
    if str_val.is_none() {
        crate::error!("getContext().fileDir is not string value");
        return None;
    }
    return str_val;
}
