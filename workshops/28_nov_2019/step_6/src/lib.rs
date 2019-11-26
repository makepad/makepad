#![allow(dead_code)]

#[macro_use]
mod macros;

mod math;
mod sierpinski;

use std::mem;

extern "C" {
    #[allow(dead_code)]
    pub fn console_log(data: u32, len: u32);
}

#[no_mangle]
pub extern "C" fn sierpinski(level: u32) -> u32 {
    unsafe { vec_f32_into_js(sierpinski::sierpinski(level)) }
}

#[no_mangle]
pub unsafe extern "C" fn free_vec_f32(raw_parts: u32) {
    let [ptr, length, capacity] = *Box::from_raw(raw_parts as *mut [u32; 3]);
    Vec::from_raw_parts(ptr as *mut f32, length as usize, capacity as usize);
}

unsafe fn vec_f32_into_js(mut vec: Vec<f32>) -> u32 {
    let raw_parts = Box::new([
        vec.as_mut_ptr() as u32,
        vec.len() as u32,
        vec.capacity() as u32,
    ]);
    mem::forget(vec);
    Box::into_raw(raw_parts) as u32
}

