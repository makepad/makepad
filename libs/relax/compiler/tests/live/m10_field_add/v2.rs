// M84: a field added to a struct whose values live only in a call's locals takes rustc's layout
// of the new text (v1: the program prints what a fresh run of v1 does, the new size too). One
// added to a struct a static holds values of restarts the app, naming the static (v2, on v1): the
// offered restart drops none of the held values (the next run prints v1's). v3 (on v1) clears the
// static: each value held since v0 drops exactly once.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

static DROPS: AtomicUsize = AtomicUsize::new(0);

struct Item {
    name: String,
    n: u32,
    done: bool,
}

impl Drop for Item {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

struct Scratch {
    label: String,
    wide: u64,
    k: u32,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { name: "a".into(), n: 10, done: false });
        items.push(Item { name: "b".into(), n: 20, done: false });
    }
    for item in items.iter() {
        println!("out {} {} {}", item.name, item.n, item.done);
    }
    println!("out drops {}", DROPS.load(Ordering::SeqCst));
    let s = Scratch { label: "s".into(), wide: 9, k: 3 };
    println!("out scratch {} {} {} {}", s.label, s.wide, s.k, std::mem::size_of::<Scratch>());
}
