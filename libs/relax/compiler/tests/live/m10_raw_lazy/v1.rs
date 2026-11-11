// M10: a builder by raw writes at offset_of! offsets that nothing calls before the edit, so it is
// first compiled after it. v1 removes a field and calls the builder: a value it makes may be kept
// where the struct's type does not show, which its new layout would read wrong -- restart, decided
// from the program, not from what was compiled.

use std::alloc::{alloc, Layout};
use std::mem::offset_of;

struct Rec {
    name: String,
    n: u32,
}

// (the allow stays though v1 calls it: a changed attribute is a changed signature, which
// restarts for its own reason before the raw writes are looked at)
#[allow(dead_code)]
fn raw(name: &str, n: u32) -> Box<Rec> {
    unsafe {
        let p = alloc(Layout::new::<Rec>());
        p.add(offset_of!(Rec, name)).cast::<String>().write(name.to_string());
        p.add(offset_of!(Rec, n)).cast::<u32>().write(n);
        Box::from_raw(p.cast::<Rec>())
    }
}

#[test]
fn run() {
    let k = Box::new(Rec { name: "k".to_string(), n: 1 });
    let r = raw("r", 7);
    println!("out {} {} {} {}", k.name, k.n, r.name, r.n);
    println!("out ok");
}
