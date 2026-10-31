// 3.12: adding a Drop impl changes needs_drop(T): every fn that drops a T (here Vec<T>::clear's
// drop glue) must be recompiled, so values made before the edit are counted when dropped after it.
// Each run clears the items the previous run pushed, prints the drop count, and pushes 3 more.
use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};

static DROPS: AtomicU32 = AtomicU32::new(0);

struct T(u32);

impl Drop for T {
    fn drop(&mut self) {
        DROPS.fetch_add(1 + self.0 * 0, Ordering::SeqCst);
    }
}

thread_local! {
    static ITEMS: RefCell<Vec<T>> = RefCell::new(Vec::new());
}

#[test]
fn run() {
    ITEMS.with(|v| {
        v.borrow_mut().clear();
        println!("out drops {}", DROPS.load(Ordering::SeqCst));
        for i in 0..3 {
            v.borrow_mut().push(T(i));
        }
    });
}
