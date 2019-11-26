#![allow(dead_code)]

#[macro_use]
mod macros;

extern "C" {
    fn console_log(data: u32, len: u32);
}

#[no_mangle]
extern "C" fn sierpinski(level: u32) -> u32 {
    println!(
        "Generating Sierpinski tetrahedron with level {} in Rust",
        level
    );
    Box::into_raw(Box::new([1, 2, 3])) as u32
}

#[no_mangle]
pub unsafe extern "C" fn free_values(values: u32) {
    Box::from_raw(values as *mut [u32; 3]);
}
