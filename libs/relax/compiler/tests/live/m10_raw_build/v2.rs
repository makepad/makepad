// M10: a struct built by raw writes at the offsets offset_of! gives (as reflection crates build
// values): the hidden ext word of its live layout is never written (here it holds 0xAB bytes),
// on the stack (MaybeUninit) and on the heap (alloc, then Box::from_raw). Built, read, dropped,
// and kept in a static next to a value of a struct literal. v1 removes a field with nothing to
// drop: the offsets the builder computes are still the running layout's, for new values and kept
// ones. v2 removes a field that owns memory: whether a raw value set it is unknown -- restart.
// v3 (on v1) adds a field the builder writes at its offset_of! offset: the running layout has no
// offset for it -- restart.

use std::alloc::{alloc, Layout};
use std::mem::{offset_of, size_of, MaybeUninit};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

struct Rec {
    tag: String,
    name: String,
    n: u32,
}

struct Lit {
    a: String,
    b: u32,
}

// owns something to drop, holds no niche
struct Note(u32);

static DROPS: AtomicU32 = AtomicU32::new(0);

impl Drop for Note {
    fn drop(&mut self) {
        DROPS.fetch_add(self.0, Ordering::SeqCst);
    }
}

static KEPT: Mutex<Vec<Rec>> = Mutex::new(Vec::new());

unsafe fn fill(p: *mut u8, name: &str, n: u32) {
    std::ptr::write_bytes(p, 0xAB, size_of::<Rec>());
    p.add(offset_of!(Rec, tag)).cast::<String>().write("t".to_string());
    p.add(offset_of!(Rec, name)).cast::<String>().write(name.to_string());
    p.add(offset_of!(Rec, n)).cast::<u32>().write(n);
}

fn on_stack(name: &str, n: u32) -> Rec {
    let mut u = MaybeUninit::<Rec>::uninit();
    unsafe {
        fill(u.as_mut_ptr().cast(), name, n);
        u.assume_init()
    }
}

fn on_heap(name: &str, n: u32) -> Box<Rec> {
    unsafe {
        let p = alloc(Layout::new::<Rec>());
        fill(p, name, n);
        Box::from_raw(p.cast::<Rec>())
    }
}

#[test]
fn run() {
    let drops = DROPS.load(Ordering::SeqCst);
    let a = on_stack("a", 7);
    println!("out {} {} {}", a.tag, a.name, a.n);
    drop(a);
    let b = on_heap("b", 8);
    println!("out {} {} {}", b.tag, b.name, b.n);
    drop(b);
    {
        // (the lock goes before code that may panic: a rerun takes it again)
        let mut kept = KEPT.lock().unwrap();
        if kept.is_empty() {
            kept.push(on_stack("k", 1));
            kept.push(Rec { tag: "lit".to_string(), name: "l".to_string(), n: 2 });
        }
        for r in kept.iter() {
            println!("out kept {} {} {}", r.tag, r.name, r.n);
        }
    }
    let l = Lit { a: "x".to_string(), b: 3 };
    println!("out lit {} {}", l.a, l.b);
    println!("out drops {}", DROPS.load(Ordering::SeqCst) - drops);
    println!("out ok");
}
