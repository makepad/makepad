//~ E0658
//~ help: allocator_ext
// rustc 1.100.0 stabilized Vec's allocator parameter but not its iterators': naming a vec::IntoIter
// with an allocator needs allocator_ext (an argument given to an `#[unstable]` parameter), as rustc
// says, while the same iterator with its type left to inference is stable.
use std::alloc::System;

fn main() {
    let mut v = Vec::new_in(System);
    v.push(1u8);
    let it: std::vec::IntoIter<u8, System> = v.into_iter();
    println!("{}", it.len());
}
