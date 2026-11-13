// Rc and Arc uninitialized and zeroed allocations, contents and reference counts.
// Differential file: `| ` lines equal under rustc --test and makepad test.

use std::rc::Rc;
use std::sync::Arc;

#[test]
fn a01_rc_arc_uninit_family() {
    let mut a = Arc::<[u8]>::new_uninit_slice(1000);
    let p = Arc::get_mut(&mut a).unwrap().as_mut_ptr().cast::<u8>();
    for i in 0..1000 {
        unsafe { *p.add(i) = (i % 251) as u8 };
    }
    let a: Arc<[u8]> = unsafe { a.assume_init() };
    let real: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
    println!("| uninit arc bytes {:?}", &a[..]);
    assert_eq!(&a[..], &real[..]);
    let b = a.clone();
    assert_eq!(Arc::strong_count(&b), 2);
    let z = unsafe { Arc::<[u64]>::new_zeroed_slice(33).assume_init() };
    assert!(z.iter().all(|x| *x == 0) && z.len() == 33);
    let mut r = Rc::<[String]>::new_uninit_slice(3);
    for (i, x) in Rc::get_mut(&mut r).unwrap().iter_mut().enumerate() {
        x.write(i.to_string());
    }
    let r = unsafe { r.assume_init() };
    assert_eq!(&r[..], &["0".to_string(), "1".to_string(), "2".to_string()][..]);
    let one = unsafe { Rc::<u128>::new_zeroed().assume_init() };
    assert_eq!(*one, 0);
    let mut u = Arc::<String>::new_uninit();
    Arc::get_mut(&mut u).unwrap().write(String::from("x"));
    let u = unsafe { u.assume_init() };
    assert_eq!(&*u, "x");
    let empty = unsafe { Rc::<[u32]>::new_uninit_slice(0).assume_init() };
    assert_eq!(empty.len(), 0);
    println!(
        "| uninit counts {} zeros {:?} strings {:?} scalar {} value {:?} empty {}",
        Arc::strong_count(&b),
        &z[..],
        &r[..],
        *one,
        &*u,
        empty.len(),
    );
}
