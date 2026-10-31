use std::sync::atomic::{AtomicPtr, Ordering};

mod payload;

struct Entry {
    name: &'static str,
    callback: fn(u32) -> u32,
}

// MAKEPAD_RUN_LAZY_DEFER=payload.rs makes this initializer wait for the module.
static ENTRIES: [Entry; 2] = [
    Entry { name: "first", callback: { use crate::payload as functions; functions::increment } },
    Entry { name: "second", callback: { use crate::payload as functions; functions::double } },
];

struct Node {
    entries: &'static [Entry],
}

// Every initializer in the reference chain must retain the dependency on
// ENTRIES until it has a value, even though its address already exists.
static NODE: Node = Node { entries: &ENTRIES };
static CATALOGUE: AtomicPtr<Node> = AtomicPtr::new(&NODE as *const Node as *mut Node);

fn main() {
    let node = unsafe { &*CATALOGUE.load(Ordering::Acquire) };
    assert_eq!(node.entries.len(), 2);
    assert_eq!(node.entries[0].name, "first");
    assert_eq!(node.entries[1].name, "second");
    assert_eq!((node.entries[0].callback)(6), 7);
    assert_eq!((node.entries[1].callback)(6), 12);
    println!("ctfe_deferred: complete catalogue");
}
