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
    const STEPS: isize = {
        let values = [0u32; 3];
        let first = core::ptr::NonNull::from_ref(&values[0]);
        let last = core::ptr::NonNull::from_ref(&values[2]);
        unsafe {
            let middle = core::ptr::NonNull::new_unchecked(last.as_ptr().wrapping_byte_offset(-4));
            middle.offset_from(first)
        }
    };
    println!("| const element offset {}", STEPS);
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
    #[allow(deprecated)]
    use core::intrinsics::{transmute, write_bytes};
    use core::ptr::NonNull;

    let mut x = [0u32; 5];
    let p = x.as_mut_ptr();
    unsafe {
        let end = p.add(4);
        println!("| mut {} {}", end.byte_offset_from(p as *const u8), end.byte_offset_from_unsigned(p as *mut u8));
        println!("| to const {}", (p as *const u32).byte_offset_from(end));
        let bytes = p as *mut u8;
        println!("| bytes {}", bytes.add(6).byte_offset_from(p.add(1)));
        let first = NonNull::new(p).unwrap();
        let last = NonNull::new(end).unwrap();
        let misaligned = first.cast::<u8>().add(1).cast::<u32>();
        println!("| aligned {} {}", first.is_aligned(), misaligned.is_aligned());
        println!("| elements {} {}", first.offset_from(last), last.offset_from_unsigned(first));
        let middle = end.wrapping_byte_offset(-8);
        let back = (middle as *const u32).wrapping_byte_offset(-4);
        println!("| wrapped {} {}", middle.offset_from(p), back.offset_from(p));
        let slot = NonNull::new(middle).unwrap();
        slot.write_volatile(57);
        println!("| volatile {} {}", slot.read_volatile(), x[2]);
        write_bytes(p.add(1), 0x11, 2);
        let signed = transmute::<[u16; 2], [i16; 2]>([0xffff, 0x7fff]);
        println!("| intrinsic {} {} {} {:?}", x[1], x[2], x[3], signed);
    }
}

#[test]
#[allow(deprecated)]
fn a04_unsized_pointees() {
    use core::{alloc::Layout, mem, ptr::NonNull};

    let s: &[u16] = &[1, 2, 3, 4];
    let whole = s as *const [u16];
    let tail = &s[2..] as *const [u16];
    let text = "relax";
    let t = &text[3..] as *const str;
    unsafe {
        println!("| slice {} str {}", tail.byte_offset_from(whole), t.byte_offset_from(text.as_ptr()));
        let shifted = tail.wrapping_byte_offset(-2);
        println!("| wrapped slice {} {}", shifted.byte_offset_from(whole), shifted.len());
        let first = NonNull::from(s);
        let last = NonNull::from(&s[2..]);
        println!("| nonnull bytes {} {}", first.byte_offset_from(last.cast::<u8>()), last.byte_offset_from_unsigned(first));
        let mut values = [1u16, 2, 3, 4];
        let part = &mut values[1..3] as *mut [u16];
        let shifted = part.wrapping_byte_offset(2);
        println!("| wrapped mut slice {} {}", shifted.byte_offset_from(part), shifted.len());
        let layout = Layout::from_size_align(7, 32).unwrap();
        println!("| dangling aligned {}", layout.dangling_ptr().addr().get() == layout.align());
        let sized = core::ptr::null::<u32>();
        println!("| raw layout {} {} {} {}", mem::size_of_val_raw(shifted), mem::align_of_val_raw(shifted),
                 mem::size_of_val_raw(sized), mem::align_of_val_raw(sized));
        println!("| minimum alignment {} {}", mem::min_align_of::<u32>(), mem::min_align_of_val(s));
    }
}

struct CountDrop<'a>(u8, &'a core::cell::Cell<usize>);

impl Clone for CountDrop<'_> {
    fn clone(&self) -> Self {
        if self.0 == 0 {
            panic!("clone stopped");
        }
        CountDrop(self.0, self.1)
    }
}

impl Drop for CountDrop<'_> {
    fn drop(&mut self) {
        self.1.set(self.1.get() + 1);
    }
}

#[test]
fn a05_maybe_uninit_slice() {
    use core::mem::MaybeUninit;

    let mut buf = [MaybeUninit::<u16>::uninit(); 3];
    let written = buf.write_copy_of_slice(&[4, 5, 6]);
    written[0] += 1;
    unsafe {
        buf.assume_init_mut()[2] += 1;
        println!("| copied {:?}", buf.assume_init_ref());
    }
    println!("| empty copy {}", buf[..0].write_copy_of_slice(&[]).len());

    let drops = core::cell::Cell::new(0);
    let src = [CountDrop(3, &drops), CountDrop(5, &drops), CountDrop(0, &drops)];
    let mut dst = [const { MaybeUninit::uninit() }; 3];
    let cloned = dst[..2].write_clone_of_slice(&src[..2]);
    println!("| cloned {} {} {}", cloned.len(), cloned[0].0, cloned[1].0);
    unsafe { dst[..2].assume_init_drop() };
    println!("| dropped {}", drops.get());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dst.write_clone_of_slice(&src);
    }));
    println!("| partial clone {} {}", result.is_err(), drops.get());
}
