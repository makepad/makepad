//~ E0277
//~ help: u32: Allocator
// Box's allocator parameter (rustc 1.100.0, relax issue 156) is bounded by Allocator: a Box in a
// type that is no allocator is rejected, as rustc rejects it.
fn main() {
    let b = Box::new_in(1u8, 5u32);
    println!("{}", *b);
}
