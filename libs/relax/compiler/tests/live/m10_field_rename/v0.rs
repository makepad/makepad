// M10: n -> count pairs by the same type and declaration position.
// The renamed slot retains the value mutated by both v0 calls.

use std::sync::Mutex;

struct Item {
    name: String,
    n: u32,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { name: "kept".into(), n: 40 });
    }
    for item in items.iter_mut() {
        println!("out {} {}", item.name, item.n);
        item.n += 1;
    }
}
