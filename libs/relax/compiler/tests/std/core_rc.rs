// std-core differential test (core_diff.sh): Rc/Arc/Weak incl. unsizing to dyn, str and slices.
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;

trait Shape {
    fn area(&self) -> u32;
}
struct Sq(u32);
impl Shape for Sq {
    fn area(&self) -> u32 {
        self.0 * self.0
    }
}

#[test]
fn a01_rc_basics() {
    let a = Rc::new(5u64);
    let b = a.clone();
    println!("| strong {} val {}", Rc::strong_count(&a), *b);
    let w = Rc::downgrade(&a);
    println!("| weak {} up {}", Rc::weak_count(&a), w.upgrade().is_some());
    drop(a);
    drop(b);
    println!("| gone {}", w.upgrade().is_none());
    let e: Weak<u32> = Weak::new();
    println!("| empty {}", e.upgrade().is_none());
    let c = Rc::new(RefCell::new(1u32));
    *c.borrow_mut() += 4;
    println!("| refcell {}", *c.borrow());
    println!("| unwrap {}", Rc::try_unwrap(Rc::new(9u8)).ok().unwrap());
}

#[test]
fn a02_unsize() {
    let s: Rc<dyn Shape> = Rc::new(Sq(3));
    let t = s.clone();
    println!("| dyn {} {}", s.area(), Rc::strong_count(&t));
    let f: Arc<dyn Fn(u32) -> u32> = Arc::new(|x| x + 1);
    println!("| arc fn {}", f(41));
    let st: Rc<str> = Rc::from("hello");
    println!("| str {} {}", st.len(), &st[1..3]);
    let sl: Arc<[u16]> = Arc::from(vec![1u16, 2, 3]);
    println!("| slice {} {}", sl.len(), sl[2]);
    let wd: Weak<dyn Shape> = Rc::downgrade(&s);
    println!("| weak dyn {}", wd.upgrade().unwrap().area());
}
