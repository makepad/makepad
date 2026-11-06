// M10: a struct built by inline assembly (as a pool's fast copy does): the builder copies a
// prototype's bytes with instructions the compiler does not read -- no field place, no cast, no
// field name in the builder; only the pointers it hands the assembly say what it writes. v1 adds
// a field: its values need the hidden ext word, which the copy takes from the prototype (two
// values, one block) -- restart.

use std::mem::{size_of, MaybeUninit};
use std::sync::Mutex;

struct Pt {
    left: u32,
    top: u32,
    depth: u32,
}

static KEPT: Mutex<Vec<Pt>> = Mutex::new(Vec::new());

// copies the bytes of `src` over `dst`
unsafe fn copy_in(dst: *mut Pt, src: *const Pt) {
    let len = size_of::<Pt>();
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!("rep movsb", inout("rcx") len => _, inout("rdi") dst => _, inout("rsi") src => _, options(nostack, preserves_flags));
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!("2:", "ldrb {t:w}, [{s}], #1", "strb {t:w}, [{d}], #1", "subs {k}, {k}, #1", "b.ne 2b",
        d = inout(reg) dst => _, s = inout(reg) src => _, k = inout(reg) len => _, t = out(reg) _, options(nostack));
}

fn make(proto: &Pt) -> Pt {
    let mut v = MaybeUninit::<Pt>::uninit();
    unsafe {
        copy_in(v.as_mut_ptr(), proto);
        v.assume_init()
    }
}

#[test]
fn run() {
    let proto = Pt { left: 3, top: 4, depth: 1 };
    let a = make(&proto);
    println!("out {} {}", a.left, a.top);
    drop(a);
    {
        // (the lock goes before code that may panic: a rerun takes it again)
        let mut kept = KEPT.lock().unwrap();
        if kept.is_empty() {
            kept.push(make(&proto));
            kept.push(Pt { left: 5, top: 6, depth: 2 });
        }
        for p in kept.iter() {
            println!("out kept {} {} {}", p.left, p.top, p.depth);
        }
    }
    drop(proto);
    println!("out ok");
}
