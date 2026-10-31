// Unsizing coercions are `Source: CoerceUnsized<Target>` goals (rustc coerce_unsized): arrays
// to slices and values to trait objects behind references, raw pointers, Box, Rc, Arc, cells
// and struct tails.
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

trait Tr { fn f(&self) -> i32; }
impl Tr for i32 { fn f(&self) -> i32 { *self } }

struct Tail<X: ?Sized> { head: i32, tail: X }

#[test]
fn unsizing() {
    let a: Box<[i32]> = Box::new([1, 2, 3]);
    assert_eq!(a.len(), 3);
    let r: Rc<dyn Tr> = Rc::new(5);
    assert_eq!(r.f(), 5);
    let ar: Arc<dyn Tr> = Arc::new(6);
    assert_eq!(ar.f(), 6);
    let cell: &RefCell<[i32]> = &RefCell::new([1, 2]);
    assert_eq!(cell.borrow().len(), 2);
    let t: &Tail<[i32]> = &Tail { head: 1, tail: [1, 2, 3] };
    assert_eq!((t.head, t.tail.len()), (1, 3));
    let raw: *const [i32] = &[1, 2];
    assert_eq!(unsafe { (*raw).len() }, 2);
    let d: &dyn Tr = &7;
    let up: &dyn Tr = d;
    assert_eq!(up.f(), 7);
}
