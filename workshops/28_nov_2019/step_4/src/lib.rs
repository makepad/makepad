#![allow(dead_code)]

#[macro_use]
mod macros;

extern "C" {
    fn console_log(data: i32, len: i32);
}

#[no_mangle]
extern "C" fn sierpinski(level: i32) {
    println!(
        "Generating Sierpinski tetrahedron with level {} in Rust",
        level
    );
}
