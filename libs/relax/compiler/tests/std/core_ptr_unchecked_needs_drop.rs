// Raw pointers' as_ref_unchecked (`*const T`, `*mut T`) and as_mut_unchecked (stable 1.95: const fns,
// #[must_use], over unsized pointees too) and mem::needs_drop for a `?Sized` type parameter (since 1.21),
// as rustc's std has them (relax issue 85). A differential file: its `| ` lines are the same under
// `rustc --test` and `makepad test`.
use std::fmt::Debug;

const fn read(p: *const u32) -> u32 {
    unsafe { *p.as_ref_unchecked() }
}

const fn bump(p: *mut u32) -> u32 {
    unsafe {
        *p.as_mut_unchecked() += 1;
        *p.as_ref_unchecked()
    }
}

const fn needs<T: ?Sized>() -> bool {
    std::mem::needs_drop::<T>()
}

const READ: u32 = read(&7);
const NEEDS: [bool; 4] = [needs::<str>(), needs::<[u8]>(), needs::<[String]>(), needs::<String>()];

#[test]
fn a01_as_ref_unchecked() {
    let x = [1u8, 2];
    let p = &x as *const [u8; 2];
    let v = unsafe { *p.as_ref_unchecked() };
    assert_eq!(v, [1, 2]);
    let s: *const str = "relax";
    let d: *const dyn Debug = &5u16;
    let mut n = 41u32;
    let after = bump(&mut n);
    let mut w = [3u32, 4];
    let ws: *mut [u32] = &mut w[..];
    unsafe {
        ws.as_mut_unchecked()[1] = 9;
    }
    let shown = unsafe { (s.as_ref_unchecked(), format!("{:?}", d.as_ref_unchecked()), ws.as_ref_unchecked().len()) };
    assert_eq!((after, n, READ, w), (42, 42, 7, [3, 9]));
    println!("| as_ref_unchecked {:?} {:?} {} {} {} {:?}", v, shown, after, n, READ, w);
}

#[test]
fn a02_needs_drop_unsized() {
    let got = [
        needs::<String>(), needs::<u8>(), needs::<str>(), needs::<[u8]>(), needs::<[String]>(),
        needs::<dyn Debug>(), needs::<(u8, [Vec<u8>])>(),
    ];
    assert_eq!(got, [true, false, false, false, true, true, true]);
    assert_eq!(NEEDS, [false, false, true, true]);
    println!("| needs_drop {:?} {:?}", got, NEEDS);
}
