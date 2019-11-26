#![allow(dead_code)]

#[macro_use]
mod macros;

extern "C" {
    fn console_log(data: u32, len: u32);
}

#[no_mangle]
extern "C" fn sierpinski(level: u32) {
    println!(
        "Generating Sierpinski tetrahedron with level {} in Rust",
        level
    );
}
