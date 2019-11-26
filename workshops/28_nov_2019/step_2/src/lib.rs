#![allow(dead_code)]

#![macro_use]
mod macros;

extern "C" {
    fn alert(level: u32);
}

#[no_mangle]
extern "C" fn sierpinski(level: u32) {
    unsafe {
        alert(level);
    }
}
