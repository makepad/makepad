#![feature(fn_static)]
#[derive(Clone, Copy)]
struct Fake;
impl std::ops::FnPtr for Fake {
    fn as_ptr(self) -> std::ptr::NonNull<std::ops::Code> { loop {} }
    unsafe fn from_ptr(_: std::ptr::NonNull<std::ops::Code>) -> Self { Fake }
}
fn main() {}
