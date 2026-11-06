// M10: a struct inside another, built by byte copies of the outer one: the builder copies a
// prototype `Bag` through `*const u8` / `*mut u8`, naming neither the inner struct nor its
// fields. v1 adds a field to the inner `Rec`: its values need the hidden ext word, which the
// copy takes from the prototype's `Rec` (two values, one block) -- restart.

use std::mem::{size_of, MaybeUninit};
use std::sync::Mutex;

struct Rec {
    label: String,
    n: u32,
    seen: bool,
}

struct Bag {
    inner: Rec,
    tag: u32,
}

static KEPT: Mutex<Vec<Bag>> = Mutex::new(Vec::new());

// a copy of `proto`, by its bytes (its label is empty: nothing owned is shared)
fn copy_of(proto: &Bag) -> Bag {
    let mut v = MaybeUninit::<Bag>::uninit();
    unsafe {
        std::ptr::copy_nonoverlapping(proto as *const Bag as *const u8, v.as_mut_ptr() as *mut u8, size_of::<Bag>());
        v.assume_init()
    }
}

#[test]
fn run() {
    let proto = Bag { inner: Rec { label: String::new(), n: 3, seen: false }, tag: 4 };
    let a = copy_of(&proto);
    println!("out {} {} {}", a.inner.label.len(), a.inner.n, a.tag);
    drop(a);
    {
        // (the lock goes before code that may panic: a rerun takes it again)
        let mut kept = KEPT.lock().unwrap();
        if kept.is_empty() {
            kept.push(Bag { inner: Rec { label: "lit".to_string(), n: 5, seen: true }, tag: 6 });
        }
        for b in kept.iter() {
            println!("out kept {} {} {} {}", b.inner.label, b.inner.n, b.inner.seen, b.tag);
        }
    }
    drop(proto);
    println!("out ok");
}
