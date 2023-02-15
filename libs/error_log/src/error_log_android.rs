use std::ffi::{c_char, c_int};
    
#[macro_export]
macro_rules!log {
    ( $ ( $ t: tt) *) => {
        crate::makepad_error_log::console_log_impl(&format!("{}:{} - {}\0", file!(), line!(), format!( $ ( $ t) *)))
    }
}

#[macro_export]
macro_rules!error {
    ( $ ( $ t: tt) *) => {
        crate::makepad_error_log::console_error_impl(&format!("{}:{} - {}\0", file!(), line!(), format!( $ ( $ t) *)))
    }
}

extern "C" {
    pub fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

pub fn console_log_impl(val: &str) {
    unsafe {
        __android_log_write(3, "Makepad\0".as_ptr(), val.as_ptr());
    }
}

extern "C" {
    pub fn js_console_error(chars: u32, len: u32);
}

pub fn console_error_impl(val: &str) { 
    unsafe {
        __android_log_write(1, "Makepad\0".as_ptr(), val.as_ptr());
    }
}
