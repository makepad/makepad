// M10: a builder by raw writes at offset_of! offsets that nothing calls before the edit, so it is
// first compiled after it. v1 removes a field and calls the builder: a value it makes may be kept
// where the struct's type does not show, which its new layout would read wrong -- restart, decided
// from the program, not from what was compiled.

use std::alloc::{alloc, Layout};
use std::mem::offset_of;

struct Rec {
    name: String,
    tag: u32,
    n: u32,
}

#[allow(dead_code)]
fn raw(name: &str, n: u32) -> Box<Rec> {
    unsafe {
        let p = alloc(Layout::new::<Rec>());
        p.add(offset_of!(Rec, name)).cast::<String>().write(name.to_string());
        p.add(offset_of!(Rec, tag)).cast::<u32>().write(n + 1);
        p.add(offset_of!(Rec, n)).cast::<u32>().write(n);
        Box::from_raw(p.cast::<Rec>())
    }
}

#[test]
fn run() {
    let k = Box::new(Rec { name: "k".to_string(), tag: 2, n: 1 });
    println!("out {} {} {}", k.name, k.tag, k.n);
    println!("out ok");
}
