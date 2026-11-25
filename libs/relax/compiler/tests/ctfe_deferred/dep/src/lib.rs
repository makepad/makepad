use std::sync::atomic::AtomicPtr;

mod payload;

pub struct Entry {
    pub name: &'static str,
    pub callback: fn(u32) -> u32,
}

// RELAX_LAZY_DEFER=payload.rs makes this initializer wait for the module. (A dependency outside the
// app's tree: the app's own crates are type-checked whole before main, which loads all their files.)
static ENTRIES: [Entry; 2] = [
    Entry { name: "first", callback: { use crate::payload as functions; functions::increment } },
    Entry { name: "second", callback: { use crate::payload as functions; functions::double } },
];

pub struct Node {
    pub entries: &'static [Entry],
}

// Every initializer in the reference chain must retain the dependency on
// ENTRIES until it has a value, even though its address already exists.
static NODE: Node = Node { entries: &ENTRIES };
pub static CATALOGUE: AtomicPtr<Node> = AtomicPtr::new(&NODE as *const Node as *mut Node);
