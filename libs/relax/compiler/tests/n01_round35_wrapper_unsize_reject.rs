//~ E0308
// T8 coercion adjustments; owner: arch (next step after N01).
// Generic wrappers do not imply CoerceUnsized. rustc rejects; Relax accepts.
use std::fmt::Display;
struct Wrapper<T: ?Sized>(*const T);
fn convert(value: Wrapper<u8>) -> Wrapper<dyn Display> { value }
#[test]
#[ignore = "T8: coercion adjustments; arch"]
fn wrapper_unsize() {
    let value = 1u8;
    let _ = convert(Wrapper(&value));
}
fn main() {}
