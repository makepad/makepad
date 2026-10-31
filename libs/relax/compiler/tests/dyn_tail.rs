// unsizing through struct tails (`Arc<Mutex<F>>` -> `Arc<Mutex<dyn Fn(u8) -> u32>>`, rustc
// CoerceUnsized + Unsize for structs), and the tail's offset from the dynamic alignment
// (vtable align): a closure capturing only a u8 sits right after the mutex's flags
use std::sync::{Arc, Mutex};
use std::rc::Rc;
use std::cell::RefCell;
struct Wrap<T: ?Sized> { tag: u8, val: T }
#[test]
fn struct_tail_unsizing() {
    let k: u8 = 3;
    let f: Arc<Mutex<dyn Fn(u8) -> u32>> = Arc::new(Mutex::new(move |x: u8| (x + k) as u32));
    assert_eq!(f.lock().unwrap()(4), 7);
    let big: u64 = 1 << 40;
    let g: Arc<Mutex<dyn Fn(u8) -> u64>> = Arc::new(Mutex::new(move |x: u8| big + x as u64));
    assert_eq!(g.lock().unwrap()(1), (1 << 40) + 1);
    let r: Rc<RefCell<dyn FnMut() -> u32>> = Rc::new(RefCell::new({ let mut n = 0u32; move || { n += 1; n } }));
    r.borrow_mut()();
    assert_eq!(r.borrow_mut()(), 2);
    let w: Box<Wrap<dyn std::fmt::Debug>> = Box::new(Wrap { tag: 9, val: 5u8 });
    assert_eq!(w.tag, 9);
    assert_eq!(format!("{:?}", &w.val), "5");
    let w2: Box<Wrap<dyn std::fmt::Debug>> = Box::new(Wrap { tag: 1, val: 7u64 });
    assert_eq!(format!("{:?}", &w2.val), "7");
}

// a literal tail (`Wrapper(&mut x, 123)`) unsizes to a trait several integer types implement:
// the obligation waits for the literal's i32 fallback (rustc issue-26709); auto traits too
struct Wrapper<'a, T: ?Sized>(&'a mut i32, #[allow(dead_code)] T);
impl<'a, T: ?Sized> Drop for Wrapper<'a, T> { fn drop(&mut self) { *self.0 = 432; } }
#[test]
fn literal_tail_unsizing() {
    let mut x = 0;
    { let w = Box::new(Wrapper(&mut x, 123)); let d: Box<Wrapper<dyn std::fmt::Debug>> = w; assert_eq!(format!("{:?}", &d.1), "123"); }
    let mut y = 0;
    { let w = Box::new(Wrapper(&mut y, 1.5)); let _: Box<Wrapper<dyn Send>> = w; }
    assert_eq!((x, y), (432, 432));
}

// a packed struct with a dyn tail: the tail's alignment is min(packed, vtable align) at run
// time (size, align and offset equal before and after unsizing); a one-field union tail
// (ManuallyDrop) is the unsized tail
#[test]
fn packed_dyn_tail() {
    #[repr(C, packed(2))]
    struct Packed<T: ?Sized>(u8, core::mem::ManuallyDrop<T>);
    let p = Packed(0, core::mem::ManuallyDrop::new(1usize));
    let p: &Packed<usize> = &p;
    let sized = (core::mem::size_of_val(p), core::mem::align_of_val(p));
    let so = unsafe { std::ptr::addr_of!(p.1).cast::<u8>().offset_from(std::ptr::addr_of!(p.0)) };
    let p: &Packed<dyn Send> = p;
    let un = (core::mem::size_of_val(p), core::mem::align_of_val(p));
    let uo = unsafe { std::ptr::addr_of!(p.1).cast::<u8>().offset_from(std::ptr::addr_of!(p.0)) };
    assert_eq!((sized, so), (un, uo));
    assert_eq!(un, (10, 2));
}

// `self: Box<Self>` on a box of a boxed trait object: autoderef passes the inner Box<dyn Tr>
trait Double { fn double(self: Box<Self>) -> usize; }
impl Double for usize { fn double(self: Box<usize>) -> usize { *self * 2 } }
#[test]
fn boxed_box_self() {
    let y: Box<dyn Double> = Box::new(3usize);
    assert_eq!(y.double(), 6);
    let x: Box<_> = Box::new(Box::new(4usize) as Box<dyn Double>);
    assert_eq!(x.double(), 8);
}

// an unsized tail behind a projection (`A::Dyn` with `type Dyn = DynRef`): references to the
// struct are fat (rustc issue-60431)
pub trait Arena { type Dyn: ?Sized; }
pub struct DynRef { _d: [()] }
pub struct RefA<A: Arena> { _v: u8, _d: A::Dyn }
pub struct Ob;
impl Arena for Ob { type Dyn = DynRef; }
#[test]
fn projection_tail() {
    assert_eq!(std::mem::size_of::<&RefA<Ob>>(), std::mem::size_of::<&[()]>());
}
