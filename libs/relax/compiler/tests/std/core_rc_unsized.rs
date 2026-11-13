// Unsized Rc and Arc copy-on-write, weak disassociation and boxed conversions.
// Differential file: `| ` lines equal under rustc --test and makepad test.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

thread_local! { static DROPS: Cell<u32> = const { Cell::new(0) }; }

#[derive(Debug, PartialEq)]
struct D(u32);
impl Clone for D {
    fn clone(&self) -> D {
        D(self.0 + 100)
    }
}
impl Drop for D {
    fn drop(&mut self) {
        DROPS.with(|d| d.set(d.get() + 1));
    }
}
fn drops() -> u32 {
    DROPS.with(|d| d.get())
}

#[test]
fn a01_make_mut_slices_and_str() {
    // shared: clone on write
    let src = [D(1), D(2), D(3)];
    let mut h: Rc<[D]> = Rc::from(&src[..]);
    let h2 = h.clone();
    let before = drops();
    Rc::make_mut(&mut h)[0].0 += 7;
    let mid = drops();
    println!("| unsized shared drops {}", mid - before);
    println!("| unsized contents {:?}", &h[..]);
    println!("| unsized original {:?}", &h2[..]);
    println!("| unsized count {}", Rc::strong_count(&h));
    // unique with weak: moves, weak dangles
    drop(h2);
    let hw = Rc::downgrade(&h);
    Rc::make_mut(&mut h)[1].0 += 1;
    println!("| unsized contents {:?}", &h[..]);
    println!("| unsized weak gone {}", hw.upgrade().is_none());
    // Arc<[u8]> and Arc<str>
    let mut a: Arc<[u8]> = Arc::from(&b"pixels"[..]);
    let a2 = a.clone();
    Arc::make_mut(&mut a)[0] = b'P';
    assert_eq!((&a[..], &a2[..]), (&b"Pixels"[..], &b"pixels"[..]));
    let mut s: Arc<str> = Arc::from("abc");
    Arc::make_mut(&mut s).make_ascii_uppercase();
    assert_eq!(&*s, "ABC");
    println!("| unsized arc {:?} {:?} {:?}", &a[..], &a2[..], &*s);
}

#[test]
fn a02_from_box() {
    let before = drops();
    let h: Rc<D> = Rc::from(Box::new(D(9)));
    assert_eq!(drops() - before, 0);
    assert_eq!(h.0, 9);
    drop(h);
    assert_eq!(drops() - before, 1);
    let a: Arc<[D]> = Arc::from(vec![D(1), D(2)].into_boxed_slice());
    assert_eq!(drops() - before, 1);
    assert_eq!(a.len(), 2);
    drop(a);
    assert_eq!(drops() - before, 3);
    let s: Rc<str> = Rc::from(String::from("boxed").into_boxed_str());
    assert_eq!(&*s, "boxed");
    let e: Arc<[u64]> = Arc::from(Vec::<u64>::new().into_boxed_slice());
    assert!(e.is_empty());
    println!("| boxed drops {} str {:?} empty {}", drops() - before, &*s, e.is_empty());
}
