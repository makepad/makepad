// M10: a struct built by byte writes (as a deserializer or a pool builds values): the builder
// copies a prototype's bytes through `*const u8` / `*mut u8` and writes each field at the byte
// offset measured on a probe value -- no field place, no offset_of!, no field name in the
// builder. v1 adds a field: a value the builder made may be kept where the struct's type does not
// show, which its new layout would read wrong -- restart.

use std::mem::{size_of, MaybeUninit};

struct Rec {
    label: String,
    n: u32,
    w: u32,
    done: bool,
}

// the byte offsets of Rec's fields, measured on a probe value
fn offsets(probe: &Rec) -> [usize; 3] {
    let base = probe as *const Rec as usize;
    [&probe.label as *const String as usize - base, &probe.n as *const u32 as usize - base, &probe.w as *const u32 as usize - base]
}

// builds into `out`: the prototype's bytes, then each value at its offset
unsafe fn build(out: *mut Rec, proto: &Rec, at: [usize; 3], s: String, a: u32, b: u32) {
    let bytes = out as *mut u8;
    std::ptr::copy_nonoverlapping(proto as *const Rec as *const u8, bytes, size_of::<Rec>());
    bytes.add(at[0]).cast::<String>().write(s);
    bytes.add(at[1]).cast::<u32>().write(a);
    bytes.add(at[2]).cast::<u32>().write(b);
}

fn make(proto: &Rec, s: &str, a: u32) -> Rec {
    let at = offsets(proto);
    let mut v = MaybeUninit::<Rec>::uninit();
    unsafe {
        build(v.as_mut_ptr(), proto, at, s.to_string(), a, a * 10);
        v.assume_init()
    }
}

#[test]
fn run() {
    let proto = Rec { label: String::new(), n: 0, w: 0, done: false };
    let a = make(&proto, "a", 7);
    println!("out {} {} {}", a.label, a.n, a.w);
    drop(a);
    drop(proto);
    println!("out ok");
}
