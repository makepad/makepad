//! Relax std Rc/Arc uninit/zeroed/assume_init vs real std (drop counts, contents, counts).
use relax_std_check::arc::Arc as HArc;
use relax_std_check::rc::Rc as HRc;
use std::mem::MaybeUninit;

#[test]
fn rc_arc_uninit_family() {
    let mut a = HArc::<[u8]>::new_uninit_slice(1000);
    let p = HArc::get_mut(&mut a).unwrap().as_mut_ptr().cast::<u8>();
    for i in 0..1000 {
        unsafe { *p.add(i) = (i % 251) as u8 };
    }
    let a: HArc<[u8]> = unsafe { a.assume_init() };
    let real: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
    assert_eq!(&a[..], &real[..]);
    let b = a.clone();
    assert_eq!(HArc::strong_count(&b), 2);
    let z = unsafe { HArc::<[u64]>::new_zeroed_slice(33).assume_init() };
    assert!(z.iter().all(|x| *x == 0) && z.len() == 33);
    let mut r = HRc::<[String]>::new_uninit_slice(3);
    for (i, x) in HRc::get_mut(&mut r).unwrap().iter_mut().enumerate() {
        x.write(i.to_string());
    }
    let r = unsafe { r.assume_init() };
    assert_eq!(&r[..], &["0".to_string(), "1".to_string(), "2".to_string()][..]);
    let one = unsafe { HRc::<u128>::new_zeroed().assume_init() };
    assert_eq!(*one, 0);
    let mut u = HArc::<String>::new_uninit();
    HArc::get_mut(&mut u).unwrap().write(String::from("x"));
    let u = unsafe { u.assume_init() };
    assert_eq!(&*u, "x");
    let _ = MaybeUninit::<u8>::uninit();
    let empty = unsafe { HRc::<[u32]>::new_uninit_slice(0).assume_init() };
    assert_eq!(empty.len(), 0);
}
