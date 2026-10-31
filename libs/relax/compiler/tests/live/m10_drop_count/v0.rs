// M10: two pre-edit items lazily initialise added Default + Drop fields.
// v1 also creates a new item with an explicit field. v2 reads all three,
// clears the Vec, and verifies exactly three drops (including a second clear).

use std::sync::Mutex;

use std::sync::atomic::{AtomicUsize, Ordering};

static DROPS: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
struct Token {
    value: u32,
}

impl Drop for Token {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

struct Item {
    name: String,
    n: u32,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { name: "a".into(), n: 10 });
        items.push(Item { name: "b".into(), n: 20 });
    }
    for item in items.iter_mut() {
        println!("out {} {}", item.name, item.n);
        item.n += 1;
    }
}
