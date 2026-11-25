use std::sync::atomic::Ordering;

use ctfe_deferred_dep::CATALOGUE;

fn main() {
    let node = unsafe { &*CATALOGUE.load(Ordering::Acquire) };
    assert_eq!(node.entries.len(), 2);
    assert_eq!(node.entries[0].name, "first");
    assert_eq!(node.entries[1].name, "second");
    assert_eq!((node.entries[0].callback)(6), 7);
    assert_eq!((node.entries[1].callback)(6), 12);
    println!("ctfe_deferred: complete catalogue");
}
