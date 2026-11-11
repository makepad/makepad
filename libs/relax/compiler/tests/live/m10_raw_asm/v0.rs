// M10: a struct built by inline assembly (as a pool's fast copy does): the builder copies a
// prototype's bytes with instructions the compiler does not read -- no field place, no cast, no
// field name in the builder; only the pointers it hands the assembly say what it writes. v1 adds
// a field: a copy may be kept where the struct's type does not show, which its new layout would
// read wrong -- restart.

use std::mem::{size_of, MaybeUninit};

struct Pt {
    left: u32,
    top: u32,
}

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
    let proto = Pt { left: 3, top: 4 };
    let a = make(&proto);
    println!("out {} {}", a.left, a.top);
    drop(a);
    drop(proto);
    println!("out ok");
}
