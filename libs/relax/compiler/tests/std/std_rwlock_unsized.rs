// matrix-sdk-common's locks and readlock lock an `RwLock<T: ?Sized>`; futures' join_all turns a
// `Box<[MaybeDone<F>]>` into a `Pin<Box<[_]>>` with `.into()`
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

struct Locked<T: ?Sized>(RwLock<T>);
impl<T: ?Sized> Locked<T> {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, T> { self.0.read().expect("poisoned") }
    fn write(&self) -> std::sync::RwLockWriteGuard<'_, T> { self.0.write().expect("poisoned") }
}

fn show<T: ?Sized + Debug>(l: &Arc<RwLock<T>>) -> String {
    let g = l.read().unwrap();
    let r: &T = &g;
    format!("{r:?} {:?} {}", l, l.is_poisoned())
}

#[test]
fn rwlock_of_unsized_locks() {
    let l: Box<Locked<[u32]>> = Box::new(Locked(RwLock::new([1u32, 2, 3])));
    l.write()[1] = 20;
    assert_eq!(&*l.read(), &[1, 20, 3]);
    let a: Arc<RwLock<[u8]>> = Arc::new(RwLock::new([7u8, 8]));
    assert_eq!(show(&a), "[7, 8] RwLock { data: [7, 8], poisoned: false, .. } false");
    let d: Arc<RwLock<dyn Debug + Send + Sync>> = Arc::new(RwLock::new(5u64));
    assert_eq!(format!("{:?}", &*d.try_read().unwrap()), "5");
    assert_eq!(RwLock::new(4).into_inner().unwrap() + 0, 4);
}

#[test]
fn boxed_slice_into_pin() {
    let b: Box<[u16]> = (0..4).collect::<Box<[_]>>();
    let p: Pin<Box<[u16]>> = b.into();
    assert_eq!(&*p, &[0, 1, 2, 3]);
    let s: Pin<Box<str>> = Box::<str>::from("hi").into();
    assert_eq!(&*s, "hi");
}
