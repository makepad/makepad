#![allow(dead_code)]

extern "C" {
    fn alert(level: i32);
}

#[no_mangle]
extern "C" fn sierpinski(level: i32) {
    unsafe {
        alert(level);
    }
}
