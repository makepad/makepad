//! Relax std Rc/Arc make_mut on unsized values and From<Box<T>> vs real std: contents, counts,
//! drop counts, clone-on-write vs move-out-from-weak behaviour.
use relax_std_check::arc::Arc as HArc;
use relax_std_check::rc::Rc as HRc;
use std::cell::Cell;

thread_local! { static DROPS: Cell<u32> = Cell::new(0); }

#[derive(Debug, PartialEq)]
struct D(u32);
impl Clone for D {
    fn clone(&self) -> D { D(self.0 + 100) }
}
impl Drop for D {
    fn drop(&mut self) { DROPS.with(|d| d.set(d.get() + 1)); }
}
fn drops() -> u32 { DROPS.with(|d| d.get()) }

#[test]
fn make_mut_slices_and_str() {
    // shared: clone on write
    let src = [D(1), D(2), D(3)];
    let mut h: HRc<[D]> = HRc::from(&src[..]);
    let mut r: std::rc::Rc<[D]> = std::rc::Rc::from(&src[..]);
    let h2 = h.clone();
    let r2 = r.clone();
    let before = drops();
    HRc::make_mut(&mut h)[0].0 += 7;
    let mid = drops();
    std::rc::Rc::make_mut(&mut r)[0].0 += 7;
    assert_eq!(mid - before, drops() - mid);
    assert_eq!(&h[..], &r[..]);
    assert_eq!(&h2[..], &r2[..]);
    assert_eq!(HRc::strong_count(&h), std::rc::Rc::strong_count(&r));
    // unique with weak: moves, weak dangles
    drop(h2);
    drop(r2);
    let hw = HRc::downgrade(&h);
    let rw = std::rc::Rc::downgrade(&r);
    HRc::make_mut(&mut h)[1].0 += 1;
    std::rc::Rc::make_mut(&mut r)[1].0 += 1;
    assert_eq!(&h[..], &r[..]);
    assert_eq!(hw.upgrade().is_none(), rw.upgrade().is_none());
    // Arc<[u8]> and Arc<str>
    let mut a: HArc<[u8]> = HArc::from(&b"pixels"[..]);
    let a2 = a.clone();
    HArc::make_mut(&mut a)[0] = b'P';
    assert_eq!((&a[..], &a2[..]), (&b"Pixels"[..], &b"pixels"[..]));
    let mut s: HArc<str> = HArc::from("abc");
    HArc::make_mut(&mut s).make_ascii_uppercase();
    assert_eq!(&*s, "ABC");
}

#[test]
fn from_box() {
    let before = drops();
    let h: HRc<D> = HRc::from(Box::new(D(9)));
    assert_eq!(drops() - before, 0);
    assert_eq!(h.0, 9);
    drop(h);
    assert_eq!(drops() - before, 1);
    let a: HArc<[D]> = HArc::from(vec![D(1), D(2)].into_boxed_slice());
    assert_eq!(drops() - before, 1);
    assert_eq!(a.len(), 2);
    drop(a);
    assert_eq!(drops() - before, 3);
    let s: HRc<str> = HRc::from(String::from("boxed").into_boxed_str());
    assert_eq!(&*s, "boxed");
    let e: HArc<[u64]> = HArc::from(Vec::<u64>::new().into_boxed_slice());
    assert!(e.is_empty());
}
