// `byte_offset_from` and `byte_offset_from_unsigned` on raw pointers (core const_ptr.rs, mut_ptr.rs,
// stable 1.75 and 1.87): the distance in bytes to a pointer of any pointee type, in a constant too.
// A field offset computed this way (an `offset_of!` of a crate's own, as interprocess 2.4.7 has) was
// E0599 here. The `*mut T` form of the unsigned one takes a `*mut U`, the signed one a `*const U`.
#[repr(C)]
struct Fields {
    a: u8,
    b: u32,
    c: [u16; 3],
    d: u64,
}

const C_AT: usize = {
    let v = std::mem::MaybeUninit::<Fields>::uninit();
    let base = v.as_ptr();
    let c = unsafe { std::ptr::addr_of!((*base).c) };
    unsafe { c.cast::<()>().byte_offset_from(base.cast::<()>()) as usize }
};

#[test]
fn a01_const_field_offset() {
    println!("| c at {} (offset_of {})", C_AT, std::mem::offset_of!(Fields, c));
}

#[test]
fn a02_mixed_pointee_types() {
    let f = Fields { a: 1, b: 2, c: [3, 4, 5], d: 6 };
    let base = &f as *const Fields;
    let b = &f.b as *const u32;
    let d = &f.d as *const u64;
    let c1 = &f.c[1] as *const u16;
    unsafe {
        println!("| b {} d {} c1 {}", b.byte_offset_from(base), d.byte_offset_from(base), c1.byte_offset_from(base));
        println!("| back {} unsigned {}", base.byte_offset_from(d), d.byte_offset_from_unsigned(b));
        println!("| field values {} {} {} {}", f.a, f.b, f.c[2], f.d);
    }
}

#[test]
fn a03_mut_pointers() {
    let mut x = [0u32; 5];
    let p = x.as_mut_ptr();
    unsafe {
        let end = p.add(4);
        println!("| mut {} {}", end.byte_offset_from(p as *const u8), end.byte_offset_from_unsigned(p as *mut u8));
        println!("| to const {}", (p as *const u32).byte_offset_from(end));
        let bytes = p as *mut u8;
        println!("| bytes {}", bytes.add(6).byte_offset_from(p.add(1)));
    }
}

#[test]
fn a04_unsized_pointees() {
    let s: &[u16] = &[1, 2, 3, 4];
    let whole = s as *const [u16];
    let tail = &s[2..] as *const [u16];
    let text = "relax";
    let t = &text[3..] as *const str;
    unsafe {
        println!("| slice {} str {}", tail.byte_offset_from(whole), t.byte_offset_from(text.as_ptr()));
    }
}
