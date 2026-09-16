#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use super::{napi_env, napi_property_descriptor, napi_status, napi_value};

pub type napi_native_binding_detach_callback = ::core::option::Option<
    unsafe extern "C" fn(
        env: napi_env,
        native_object: *mut ::core::ffi::c_void,
        hint: *mut ::core::ffi::c_void,
    ) -> *mut ::core::ffi::c_void,
>;
pub type napi_native_binding_attach_callback = ::core::option::Option<
    unsafe extern "C" fn(
        env: napi_env,
        native_object: *mut ::core::ffi::c_void,
        hint: *mut ::core::ffi::c_void,
    ) -> napi_value,
>;

extern "C" {
    pub fn napi_load_module(
        env: napi_env,
        path: *const ::core::ffi::c_char,
        result: *mut napi_value,
    ) -> napi_status;

    pub fn napi_create_object_with_properties(
        env: napi_env,
        result: *mut napi_value,
        property_count: usize,
        properties: *const napi_property_descriptor,
    ) -> napi_status;

    pub fn napi_create_object_with_named_properties(
        env: napi_env,
        result: *mut napi_value,
        property_count: usize,
        keys: *mut *const ::core::ffi::c_char,
        values: *const napi_value,
    ) -> napi_status;

    pub fn napi_coerce_to_native_binding_object(
        env: napi_env,
        js_object: napi_value,
        detach_cb: napi_native_binding_detach_callback,
        attach_cb: napi_native_binding_attach_callback,
        native_object: *mut ::core::ffi::c_void,
        hint: *mut ::core::ffi::c_void,
    ) -> napi_status;
}
