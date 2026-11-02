//~ E0308
// A generic wrapper of a raw pointer has no CoerceUnsized impl: rustc rejects the coercion.
use std::fmt::Display;
struct Wrapper<T: ?Sized>(*const T);
fn convert(value: Wrapper<u8>) -> Wrapper<dyn Display> { value }
#[test]
fn wrapper_unsize() {
    let value = 1u8;
    let _ = convert(Wrapper(&value));
}
fn main() {}
