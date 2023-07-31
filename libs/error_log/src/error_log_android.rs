use std::panic;
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
    pub fn __android_log_write(prio: c_int, tag: *const u8, text: *const u8) -> c_int;
}

pub fn console_log_impl(val: &str) {
    unsafe {
        __android_log_write(3, "Makepad\0".as_ptr(), val.as_ptr());
    }
}

pub fn console_error_impl(val: &str) { 
    unsafe {
        __android_log_write(3, "Makepad\0".as_ptr(), val.as_ptr());
    }
}

pub fn init_panic_hook() {
    pub fn panic_hook(info: &panic::PanicInfo) {
        let msg = format!("Panic - {}\0", info);
        unsafe{__android_log_write(3, "Makepad\0".as_ptr(), msg.as_ptr())};
    }
    panic::set_hook(Box::new(panic_hook));
}

