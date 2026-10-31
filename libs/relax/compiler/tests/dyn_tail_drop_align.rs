// Dropping an unsized struct must project its tail using dynamic alignment,
// both through generated Arc drop glue and Box's drop_in_place intrinsic.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

static DROPPED: AtomicUsize = AtomicUsize::new(0);
static DROPS: AtomicUsize = AtomicUsize::new(0);

trait Marker {}

#[repr(align(64))]
struct Aligned {
    _pad: [u8; 64],
}

impl Marker for Aligned {}

impl Drop for Aligned {
    fn drop(&mut self) {
        DROPPED.store(self as *mut Self as usize, Ordering::SeqCst);
        DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

struct Tail<T: ?Sized> {
    _tag: u8,
    value: T,
}

#[test]
fn aligned_dyn_tail() {
    let value = Arc::new(Tail { _tag: 1, value: Aligned { _pad: [0; 64] } });
    let expected = &value.value as *const Aligned as usize;
    let erased: Arc<Tail<dyn Marker>> = value;
    drop(erased);
    assert_eq!(DROPPED.load(Ordering::SeqCst), expected);
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);

    let value = Box::new(Tail { _tag: 1, value: Aligned { _pad: [0; 64] } });
    let expected = &value.value as *const Aligned as usize;
    let erased: Box<Tail<dyn Marker>> = value;
    drop(erased);
    assert_eq!(DROPPED.load(Ordering::SeqCst), expected);
    assert_eq!(DROPS.load(Ordering::SeqCst), 2);
}
