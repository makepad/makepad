#![allow(dead_code)]

#[macro_use]
mod macros;

extern "C" {
    fn console_log(data: i32, len: i32);
}

#[no_mangle]
extern "C" fn sierpinski(level: i32) -> i32 {
    println!(
        "Generating Sierpinski tetrahedron with level {} in Rust",
        level
    );
    Box::into_raw(Box::new([1, 2, 3])) as i32
}

#[no_mangle]
pub unsafe extern "C" fn free_values(values: i32) {
    Box::from_raw(values as *mut [i32; 3]);
}
