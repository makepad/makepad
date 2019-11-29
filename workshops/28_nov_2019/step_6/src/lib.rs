#![allow(dead_code)]

#[macro_use]
mod macros;

mod math;

use std::mem;

extern "C" {
    pub fn console_log(data: i32, len: i32);
}

#[no_mangle]
pub extern "C" fn sierpinski(level: i32) -> i32 {
    println!(
        "Generating Sierpinski tetrahedron with level {} in Rust",
        level
    );
    return unsafe {
        vec_f32_into_js(vec![
            -0.5, -0.5, 0.0, 0.5, -0.5, 0.0, -0.5, 0.5, 0.0, -0.5, 0.5, 0.0, 0.5, -0.5, 0.0, 0.5,
            0.5, 0.0,
        ])
 
    };
}

#[no_mangle]
pub unsafe extern "C" fn free_vec_f32(raw_parts: i32) {
    let [ptr, length, capacity] = *Box::from_raw(raw_parts as *mut [i32; 3]);
    Vec::from_raw_parts(ptr as *mut f32, length as usize, capacity as usize);
}

pub unsafe extern "C" fn vec_f32_into_js(mut vec: Vec<f32>) -> i32 {
    let raw_parts = Box::new([
        vec.as_mut_ptr() as i32,
        vec.len() as i32,
        vec.capacity() as i32,
    ]);
    mem::forget(vec);
    Box::into_raw(raw_parts) as i32
}
