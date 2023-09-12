use std::panic;
use std::ffi::c_int;
pub use crate::LogType;

pub fn log_with_type(file:&str, line_start:u32, column_start:u32, _line_end:u32, _column_end:u32, message:&str, _ty:LogType){
    let msg = format!("{}:{}:{} - {}\0", file, line_start, column_start, message);
    unsafe{__android_log_write(3, "Makepad\0".as_ptr(), msg.as_ptr())};
}

extern "C" { 
    pub fn __android_log_write(prio: c_int, tag: *const u8, text: *const u8) -> c_int;
}


pub fn init_panic_hook() {
    pub fn panic_hook(info: &panic::PanicInfo) {
        log_with_type("",0,0,0,0,&format!("Panic - {}\0", info), LogType::Panic);
    }
    panic::set_hook(Box::new(panic_hook));
}
