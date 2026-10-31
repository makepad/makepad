// M10: Copy structs cannot safely share extension blocks.
// v1 adds a field and must request restart. v0 stabilises after its first
// mutation so a rejected edit rerun has exactly the previous output.

use std::sync::Mutex;

#[derive(Clone, Copy)]
struct Item {
    n: u32,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { n: 10 });
    }
    for item in items.iter_mut() {
        println!("out n {}", item.n);
        item.n = 11;
    }
}
