// `rc::Weak::into_raw` and `from_raw` (std since 1.45): the weak count travels with the value's
// pointer, as a `Weak` passed through a C context pointer does (winit's macOS run-loop observer:
// E0599 here, so no winit app opened a window). sync::Weak had them already.
use std::rc::{Rc, Weak};

#[test]
fn a01_round_trip_keeps_the_count() {
    let rc = Rc::new(41u32);
    let raw = Weak::into_raw(Rc::downgrade(&rc));
    println!("| weak {} strong {} value {}", Rc::weak_count(&rc), Rc::strong_count(&rc), unsafe { *raw });
    println!("| same address {}", std::ptr::eq(raw, &*rc));
    let back = unsafe { Weak::from_raw(raw) };
    println!("| upgraded {:?}", back.upgrade().map(|v| *v + 1));
    drop(back);
    println!("| weak after drop {}", Rc::weak_count(&rc));
}

#[test]
fn a02_dead_and_empty() {
    let rc = Rc::new(String::from("gone"));
    let raw = Rc::downgrade(&rc).into_raw();
    drop(rc);
    let back = unsafe { Weak::from_raw(raw) };
    println!("| dead {} strong {} weak {}", back.upgrade().is_none(), back.strong_count(), back.weak_count());
    let empty = unsafe { Weak::<u8>::from_raw(Weak::<u8>::new().into_raw()) };
    println!("| empty {} {}", empty.upgrade().is_none(), empty.strong_count());
}

trait Named {
    fn name(&self) -> String;
}
struct Plug(u8);
impl Named for Plug {
    fn name(&self) -> String {
        format!("plug {}", self.0)
    }
}

#[test]
fn a03_unsized() {
    let rc: Rc<dyn Named> = Rc::new(Plug(3));
    let raw: *const dyn Named = Rc::downgrade(&rc).into_raw();
    let back: Weak<dyn Named> = unsafe { Weak::from_raw(raw) };
    println!("| {:?} weak {}", back.upgrade().map(|n| n.name()), Rc::weak_count(&rc));
    let s: Rc<str> = Rc::from("abc");
    let w = unsafe { Weak::from_raw(Rc::downgrade(&s).into_raw()) };
    println!("| {:?}", w.upgrade().as_deref());
}
