// C that calls back into this crate's Rust, as a -sys crate's C calls its crate's `#[no_mangle]` fn.
#[no_mangle]
pub extern "C" fn snd_rust_side(x: i32) -> i32 {
    x * 10
}
extern "C" {
    fn snd_c_entry(x: i32) -> i32;
}
pub fn through_c(x: i32) -> i32 {
    unsafe { snd_c_entry(x) }
}
